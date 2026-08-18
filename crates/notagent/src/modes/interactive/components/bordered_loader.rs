//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/bordered-loader.ts` (68 LOC).

use std::rc::Rc;

use notagent_tui::components::cancellable_loader::CancellableLoader;
use notagent_tui::components::loader::Loader;
use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::tui::{Component, Container, Line, component_ref};
use tokio_util::sync::CancellationToken;

use crate::modes::interactive::theme::theme::{Theme, ThemeColor};

use super::dynamic_border::DynamicBorder;
use super::keybinding_hints::key_hint;

/// Either loader variant the bordered loader wraps.
enum WrappedLoader {
    Cancellable(Rc<std::cell::RefCell<CancellableLoader>>),
    Plain(Rc<std::cell::RefCell<Loader>>),
}

/// Loader wrapped with borders.
pub struct BorderedLoader {
    container: Container,
    loader: WrappedLoader,
    cancellable: bool,
    /// Set for the non-cancellable variant, which TypeScript backs with an
    /// `AbortController` that is never tripped.
    signal_token: Option<CancellationToken>,
}

impl BorderedLoader {
    /// New bordered loader; `cancellable` defaults to `true` in TypeScript.
    pub fn new(theme: &Theme, message: impl Into<String>, cancellable: Option<bool>) -> Self {
        let cancellable = cancellable.unwrap_or(true);
        let mut container = Container::new();
        let border_color: Rc<dyn Fn(&str) -> String> = {
            let ansi = theme.get_fg_ansi(ThemeColor::Border).to_string();
            Rc::new(move |text: &str| format!("{ansi}{text}\x1b[39m"))
        };
        let accent: Rc<dyn Fn(&str) -> String> = {
            let ansi = theme.get_fg_ansi(ThemeColor::Accent).to_string();
            Rc::new(move |text: &str| format!("{ansi}{text}\x1b[39m"))
        };
        let muted: Rc<dyn Fn(&str) -> String> = {
            let ansi = theme.get_fg_ansi(ThemeColor::Muted).to_string();
            Rc::new(move |text: &str| format!("{ansi}{text}\x1b[39m"))
        };

        container.add_child(component_ref(DynamicBorder::new(Some(Rc::clone(
            &border_color,
        )))));

        let (loader, signal_token) = if cancellable {
            let loader = Rc::new(std::cell::RefCell::new(CancellableLoader::new(
                accent, muted, message, None,
            )));
            (WrappedLoader::Cancellable(loader), None)
        } else {
            let loader = Rc::new(std::cell::RefCell::new(Loader::new(
                accent, muted, message, None,
            )));
            (WrappedLoader::Plain(loader), Some(CancellationToken::new()))
        };
        match &loader {
            WrappedLoader::Cancellable(inner) => container.add_child(Rc::clone(inner) as _),
            WrappedLoader::Plain(inner) => container.add_child(Rc::clone(inner) as _),
        }

        if cancellable {
            container.add_child(component_ref(Spacer::new(1)));
            container.add_child(component_ref(Text::new(
                key_hint("tui.select.cancel", "cancel"),
                1,
                0,
            )));
        }
        container.add_child(component_ref(Spacer::new(1)));
        container.add_child(component_ref(DynamicBorder::new(Some(border_color))));

        Self {
            container,
            loader,
            cancellable,
            signal_token,
        }
    }

    /// Token that trips when the user cancels.
    pub fn signal(&self) -> CancellationToken {
        match (&self.loader, &self.signal_token) {
            (WrappedLoader::Cancellable(loader), _) => loader.borrow().token(),
            (_, Some(token)) => token.clone(),
            // TypeScript falls back to a fresh, never-aborted controller.
            _ => CancellationToken::new(),
        }
    }

    /// Register the cancel callback (ignored by the non-cancellable variant).
    pub fn set_on_abort(&mut self, on_abort: Option<Box<dyn FnMut()>>) {
        if let WrappedLoader::Cancellable(loader) = &self.loader {
            loader.borrow_mut().on_abort = on_abort;
        }
    }

    /// Stop the animation.
    pub fn dispose(&mut self) {
        match &self.loader {
            WrappedLoader::Cancellable(loader) => loader.borrow_mut().dispose(),
            WrappedLoader::Plain(loader) => loader.borrow_mut().stop(),
        }
    }
}

impl Component for BorderedLoader {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.container.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        if self.cancellable
            && let WrappedLoader::Cancellable(loader) = &self.loader
        {
            loader.borrow_mut().handle_input(data);
        }
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }
}
