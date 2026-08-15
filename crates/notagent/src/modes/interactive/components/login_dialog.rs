//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/login-dialog.ts` (233 LOC).
//!
//! The dialog that replaces the editor while an OAuth login runs: it shows the
//! verification URL, opens the browser, and collects the code the provider asks
//! for.
//!
//! Deviations:
//!   * Class 3: `AbortController`/`AbortSignal` → `CancellationToken`; the
//!     `Promise` of `showPrompt`/`showManualInput` → `oneshot::Receiver`, which
//!     carries the same "resolve once, reject on cancel" semantics.
//!   * Class 1: `tui.requestRender()` → an injected `Rc<dyn Fn()>`, as in the
//!     other ported dialogs; `input.onSubmit`/`onEscape` set a flag that
//!     `handle_input` reads right after `Input::handle_input`, because the
//!     callbacks cannot re-enter the dialog that owns the input.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use notagent_ai::auth::types::AuthInfoLink;
use notagent_ai::compat::extension_oauth_types::OAuthDeviceCodeInfo;
use notagent_tui::components::input::Input;
use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::keybindings::keybindings_match;
use notagent_tui::tui::{Component, ComponentRef, Container, Focusable, component_ref};

use crate::modes::interactive::theme::theme::{ThemeColor, theme};

use super::dynamic_border::DynamicBorder;
use super::keybinding_hints::key_hint;

/// The rejection of `showPrompt`/`showManualInput`: `new Error("Login cancelled")`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoginCancelled;

impl std::fmt::Display for LoginCancelled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Login cancelled")
    }
}

impl std::error::Error for LoginCancelled {}

/// The answer of an input step, or the cancellation that ended it.
pub type LoginInput = oneshot::Receiver<Result<String, LoginCancelled>>;

/// An OSC 8 hyperlink, spelled out as in the TypeScript source.
fn hyperlink(url: &str, text: &str) -> String {
    format!("\x1b]8;;{url}\x07{text}\x1b]8;;\x07")
}

fn click_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "Cmd+click to open"
    } else {
        "Ctrl+click to open"
    }
}

/// Login dialog component - replaces editor during OAuth login flow
pub struct LoginDialogComponent {
    container: Container,
    content_container: Rc<RefCell<Container>>,
    input: Rc<RefCell<Input>>,
    input_ref: ComponentRef,
    request_render: Rc<dyn Fn()>,
    cancellation: CancellationToken,
    /// `inputResolver`/`inputRejecter` — one sender serves both.
    pending: Option<oneshot::Sender<Result<String, LoginCancelled>>>,
    submitted: Rc<Cell<bool>>,
    escaped: Rc<Cell<bool>>,
    on_complete: Box<dyn FnMut(bool, Option<String>)>,
    focused: bool,
}

impl LoginDialogComponent {
    pub fn new(
        request_render: Rc<dyn Fn()>,
        provider_id: &str,
        on_complete: Box<dyn FnMut(bool, Option<String>)>,
        provider_name_override: Option<&str>,
        title_override: Option<&str>,
    ) -> Self {
        let theme_instance = theme();
        let mut container = Container::new();

        let provider_name = provider_name_override
            .filter(|name| !name.is_empty())
            .unwrap_or(provider_id);
        let title = title_override
            .map(str::to_string)
            .unwrap_or_else(|| format!("Login to {provider_name}"));

        // Top border
        container.add_child(component_ref(DynamicBorder::new(None)));

        // Title
        container.add_child(component_ref(Text::new(
            theme_instance.fg(ThemeColor::Accent, &theme_instance.bold(&title)),
            1,
            0,
        )));

        // Dynamic content area
        let content_container = Rc::new(RefCell::new(Container::new()));
        container.add_child(Rc::clone(&content_container) as ComponentRef);

        // Input (always present, used when needed)
        let submitted = Rc::new(Cell::new(false));
        let escaped = Rc::new(Cell::new(false));
        let mut input = Input::new();
        {
            let flag = Rc::clone(&submitted);
            input.on_submit = Some(Box::new(move |_value| flag.set(true)));
            let flag = Rc::clone(&escaped);
            input.on_escape = Some(Box::new(move || flag.set(true)));
        }
        let input = Rc::new(RefCell::new(input));
        let input_ref = Rc::clone(&input) as ComponentRef;

        // Bottom border
        container.add_child(component_ref(DynamicBorder::new(None)));

        Self {
            container,
            content_container,
            input,
            input_ref,
            request_render,
            cancellation: CancellationToken::new(),
            pending: None,
            submitted,
            escaped,
            on_complete,
            focused: false,
        }
    }

    /// `get signal()`.
    pub fn signal(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    fn replace_input_with_submitted_text(&mut self, value: &str) {
        let replacement = component_ref(Text::new(format!("> {value}"), 0, 0));
        let mut content = self.content_container.borrow_mut();
        for child in content.children.iter_mut() {
            if Rc::ptr_eq(child, &self.input_ref) {
                *child = Rc::clone(&replacement);
            }
        }
    }

    /// The `onSubmit` half of the input, run after `Input::handle_input`.
    fn submit(&mut self) {
        if let Some(resolver) = self.pending.take() {
            let value = self.input.borrow().get_value().to_string();
            self.replace_input_with_submitted_text(&value);
            let _ = resolver.send(Ok(value));
        }
    }

    fn cancel(&mut self) {
        self.cancellation.cancel();
        if let Some(rejecter) = self.pending.take() {
            let _ = rejecter.send(Err(LoginCancelled));
        }
        (self.on_complete)(false, Some("Login cancelled".to_string()));
    }

    /// Called by onAuth callback - show URL and optional instructions
    pub fn show_auth(&mut self, url: &str, instructions: Option<&str>) {
        let theme_instance = theme();
        {
            let mut content = self.content_container.borrow_mut();
            content.clear();
            content.add_child(component_ref(Spacer::new(1)));
            content.add_child(component_ref(Text::new(
                theme_instance.fg(ThemeColor::Accent, &hyperlink(url, url)),
                1,
                0,
            )));

            content.add_child(component_ref(Text::new(
                theme_instance.fg(ThemeColor::Dim, &hyperlink(url, click_hint())),
                1,
                0,
            )));

            if let Some(instructions) = instructions {
                content.add_child(component_ref(Spacer::new(1)));
                content.add_child(component_ref(Text::new(
                    theme_instance.fg(ThemeColor::Warning, instructions),
                    1,
                    0,
                )));
            }
        }

        crate::utils::open_browser::open_browser(url);
        (self.request_render)();
    }

    /// Called by onDeviceCode callback - show URL and user code.
    pub fn show_device_code(&mut self, info: &OAuthDeviceCodeInfo) {
        let theme_instance = theme();
        {
            let mut content = self.content_container.borrow_mut();
            content.clear();
            content.add_child(component_ref(Spacer::new(1)));
            content.add_child(component_ref(Text::new(
                theme_instance.fg(
                    ThemeColor::Accent,
                    &hyperlink(&info.verification_uri, &info.verification_uri),
                ),
                1,
                0,
            )));

            content.add_child(component_ref(Text::new(
                theme_instance.fg(
                    ThemeColor::Dim,
                    &hyperlink(&info.verification_uri, click_hint()),
                ),
                1,
                0,
            )));
            content.add_child(component_ref(Spacer::new(1)));
            content.add_child(component_ref(Text::new(
                theme_instance.fg(
                    ThemeColor::Warning,
                    &format!("Enter code: {}", info.user_code),
                ),
                1,
                0,
            )));
        }

        (self.request_render)();
    }

    /// Show input for manual code/URL entry (for callback server providers)
    pub fn show_manual_input(&mut self, prompt: &str) -> LoginInput {
        let theme_instance = theme();
        self.input.borrow_mut().set_value("");
        {
            let mut content = self.content_container.borrow_mut();
            content.add_child(component_ref(Spacer::new(1)));
            content.add_child(component_ref(Text::new(
                theme_instance.fg(ThemeColor::Dim, prompt),
                1,
                0,
            )));
            content.add_child(Rc::clone(&self.input_ref));
            content.add_child(component_ref(Text::new(
                format!("({})", key_hint("tui.select.cancel", "to cancel")),
                1,
                0,
            )));
        }
        (self.request_render)();

        let (sender, receiver) = oneshot::channel();
        self.pending = Some(sender);
        receiver
    }

    /// Called by onPrompt callback - show prompt and wait for input
    /// Note: Does NOT clear content, appends to existing (preserves URL from showAuth)
    pub fn show_prompt(&mut self, message: &str, placeholder: Option<&str>) -> LoginInput {
        let theme_instance = theme();
        {
            let mut content = self.content_container.borrow_mut();
            content.add_child(component_ref(Spacer::new(1)));
            content.add_child(component_ref(Text::new(
                theme_instance.fg(ThemeColor::Text, message),
                1,
                0,
            )));
            if let Some(placeholder) = placeholder {
                content.add_child(component_ref(Text::new(
                    theme_instance.fg(ThemeColor::Dim, &format!("e.g., {placeholder}")),
                    1,
                    0,
                )));
            }
            content.add_child(Rc::clone(&self.input_ref));
            content.add_child(component_ref(Text::new(
                format!(
                    "({} {})",
                    key_hint("tui.select.cancel", "to cancel,"),
                    key_hint("tui.select.confirm", "to submit")
                ),
                1,
                0,
            )));
        }

        self.input.borrow_mut().set_value("");
        (self.request_render)();

        let (sender, receiver) = oneshot::channel();
        self.pending = Some(sender);
        receiver
    }

    /// Show informational text before another login step.
    pub fn show_details(&mut self, lines: &[String]) {
        {
            let mut content = self.content_container.borrow_mut();
            content.clear();
            content.add_child(component_ref(Spacer::new(1)));
            for line in lines {
                content.add_child(component_ref(Text::new(line.clone(), 1, 0)));
            }
        }
        (self.request_render)();
    }

    /// Show provider-owned information and links without starting an auth callback flow.
    pub fn show_info(&mut self, message: &str, links: &[AuthInfoLink], show_close_hint: bool) {
        let theme_instance = theme();
        {
            let mut content = self.content_container.borrow_mut();
            content.add_child(component_ref(Spacer::new(1)));
            content.add_child(component_ref(Text::new(
                theme_instance.fg(ThemeColor::Text, message),
                1,
                0,
            )));
            for link in links {
                let text = match link.label.as_deref() {
                    Some(label) if !label.is_empty() => format!("{label}: {}", link.url),
                    _ => link.url.clone(),
                };
                content.add_child(component_ref(Text::new(
                    theme_instance.fg(ThemeColor::Accent, &hyperlink(&link.url, &text)),
                    1,
                    0,
                )));
            }
            if show_close_hint {
                content.add_child(component_ref(Spacer::new(1)));
                content.add_child(component_ref(Text::new(
                    format!("({})", key_hint("tui.select.cancel", "to close")),
                    1,
                    0,
                )));
            }
        }
        (self.request_render)();
    }

    /// Show waiting message (for polling flows like GitHub Copilot)
    pub fn show_waiting(&mut self, message: &str) {
        let theme_instance = theme();
        {
            let mut content = self.content_container.borrow_mut();
            content.add_child(component_ref(Spacer::new(1)));
            content.add_child(component_ref(Text::new(
                theme_instance.fg(ThemeColor::Dim, message),
                1,
                0,
            )));
            content.add_child(component_ref(Text::new(
                format!("({})", key_hint("tui.select.cancel", "to cancel")),
                1,
                0,
            )));
        }
        (self.request_render)();
    }

    /// Called by onProgress callback
    pub fn show_progress(&mut self, message: &str) {
        {
            let mut content = self.content_container.borrow_mut();
            content.add_child(component_ref(Text::new(
                theme().fg(ThemeColor::Dim, message),
                1,
                0,
            )));
        }
        (self.request_render)();
    }
}

impl Component for LoginDialogComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.container.render(width)
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }

    fn handle_input(&mut self, data: &str) {
        if keybindings_match(data, "tui.select.cancel") {
            self.cancel();
            return;
        }

        // Pass to input
        self.input.borrow_mut().handle_input(data);
        if self.submitted.replace(false) {
            self.submit();
        }
        if self.escaped.replace(false) {
            self.cancel();
        }
    }

    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        Some(self)
    }
}

// Focusable implementation - propagate to input for IME cursor positioning
impl Focusable for LoginDialogComponent {
    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.input.borrow_mut().set_focused(focused);
    }
}
