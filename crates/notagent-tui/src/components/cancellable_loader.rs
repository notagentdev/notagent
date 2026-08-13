//! Loader that can be cancelled with Escape.
//!
//! 1:1 port of `packages/tui/src/components/cancellable-loader.ts` (40 LOC).
//! `AbortController` becomes a `CancellationToken` (master plan substitution).

use tokio_util::sync::CancellationToken;

use crate::components::loader::{ColorFn, Loader, LoaderIndicatorOptions};
use crate::keybindings::keybindings_match;
use crate::tui::Component;

/// Loader with a cancellation token that trips on Escape.
pub struct CancellableLoader {
    loader: Loader,
    token: CancellationToken,
    /// Called when the user presses Escape.
    pub on_abort: Option<Box<dyn FnMut()>>,
}

impl CancellableLoader {
    /// New cancellable loader.
    pub fn new(
        spinner_color_fn: ColorFn,
        message_color_fn: ColorFn,
        message: impl Into<String>,
        indicator: Option<LoaderIndicatorOptions>,
    ) -> Self {
        Self {
            loader: Loader::new(spinner_color_fn, message_color_fn, message, indicator),
            token: CancellationToken::new(),
            on_abort: None,
        }
    }

    /// Token that is cancelled when the user presses Escape.
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    /// Whether the loader was aborted.
    pub fn aborted(&self) -> bool {
        self.token.is_cancelled()
    }

    /// The underlying loader (message, indicator, frame deadline).
    pub fn loader_mut(&mut self) -> &mut Loader {
        &mut self.loader
    }

    /// Stop the animation.
    pub fn dispose(&mut self) {
        self.loader.stop();
    }
}

impl Component for CancellableLoader {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.loader.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        if keybindings_match(data, "tui.select.cancel") {
            self.token.cancel();
            if let Some(callback) = self.on_abort.as_mut() {
                callback();
            }
        }
    }

    fn invalidate(&mut self) {
        self.loader.invalidate();
    }
}
