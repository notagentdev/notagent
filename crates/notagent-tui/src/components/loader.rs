use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::components::shimmer::{SHIMMER_FRAME_MS, ShimmerPalette, shimmer};
use crate::components::text::Text;
use crate::tui::{Component, Line};
use crate::utils::{slice_by_column, visible_width};

const DEFAULT_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const DEFAULT_INTERVAL_MS: u64 = 80;

/// Animation options of a [`Loader`].
#[derive(Debug, Clone, Default)]
pub struct LoaderIndicatorOptions {
    /// Animation frames; an empty vector hides the indicator.
    pub frames: Option<Vec<String>>,
    /// Frame interval in milliseconds.
    pub interval_ms: Option<u64>,
}

/// Colouring function for spinner and message.
pub type ColorFn = Rc<dyn Fn(&str) -> String>;

/// Spinner plus message; the frame advances on [`Loader::tick`].
pub struct Loader {
    text: Text,
    frames: Vec<String>,
    interval_ms: u64,
    current_frame: usize,
    next_frame_at: Option<Instant>,
    render_indicator_verbatim: bool,
    spinner_color_fn: ColorFn,
    message_color_fn: ColorFn,
    message: String,
    message_prefix: String,
    rendered_message_prefix: String,
    /// Appended after the message, verbatim: the caller styles it, and the
    /// shimmer never touches it. A running clock beside an animated message
    /// stays still — motion on a figure reads as the figure changing.
    suffix: String,
    render_requested: bool,
    /// When set, the message carries the animation itself and no spinner is
    /// drawn. `None` inside it means the terminal cannot show the fade, in
    /// which case the message is simply left still.
    shimmer: Option<Option<ShimmerPalette>>,
    started_at: Instant,
}

impl Loader {
    pub fn new(
        spinner_color_fn: ColorFn,
        message_color_fn: ColorFn,
        message: impl Into<String>,
        indicator: Option<LoaderIndicatorOptions>,
    ) -> Self {
        let mut loader = Self {
            text: Text::new("", 1, 0),
            frames: DEFAULT_FRAMES
                .iter()
                .map(|frame| (*frame).to_string())
                .collect(),
            interval_ms: DEFAULT_INTERVAL_MS,
            current_frame: 0,
            next_frame_at: None,
            render_indicator_verbatim: false,
            spinner_color_fn,
            message_color_fn,
            message: message.into(),
            message_prefix: String::new(),
            rendered_message_prefix: String::new(),
            suffix: String::new(),
            render_requested: false,
            shimmer: None,
            started_at: Instant::now(),
        };
        loader.set_indicator(indicator);
        loader
    }

    /// Animate the message itself instead of drawing a spinner beside it.
    /// `palette` is `None` when the terminal cannot render the fade; the
    /// spinner still goes, and the message is drawn once and left alone.
    pub fn set_shimmer(&mut self, palette: Option<ShimmerPalette>) {
        self.shimmer = Some(palette);
        self.started_at = Instant::now();
        self.start();
    }

    fn is_animating(&self) -> bool {
        match self.shimmer {
            Some(palette) => palette.is_some(),
            None => self.frames.len() > 1,
        }
    }

    fn frame_interval_ms(&self) -> u64 {
        match self.shimmer {
            Some(_) => SHIMMER_FRAME_MS,
            None => self.interval_ms,
        }
    }

    /// Start the animation.
    pub fn start(&mut self) {
        self.update_display();
        self.restart_animation();
    }

    /// Stop the animation.
    pub fn stop(&mut self) {
        self.next_frame_at = None;
    }

    /// Replace the message.
    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = message.into();
        self.update_display();
    }

    /// A prefix animated together with the message but placed in the gutter by
    /// the caller, so neither the text position nor its wrapping changes.
    pub fn set_message_prefix(&mut self, prefix: impl Into<String>) {
        self.message_prefix = prefix.into();
        self.update_display();
    }

    pub fn rendered_message_prefix(&self) -> &str {
        &self.rendered_message_prefix
    }

    /// Replace the pre-styled text appended after the message.
    pub fn set_suffix(&mut self, suffix: impl Into<String>) {
        self.suffix = suffix.into();
        self.update_display();
    }

    /// Replace the colouring of the message.
    pub fn set_message_color(&mut self, message_color_fn: ColorFn) {
        self.message_color_fn = message_color_fn;
        self.update_display();
    }

    /// Replace the indicator options.
    pub fn set_indicator(&mut self, indicator: Option<LoaderIndicatorOptions>) {
        self.render_indicator_verbatim = indicator.is_some();
        self.frames = indicator
            .as_ref()
            .and_then(|options| options.frames.clone())
            .unwrap_or_else(|| {
                DEFAULT_FRAMES
                    .iter()
                    .map(|frame| (*frame).to_string())
                    .collect()
            });
        self.interval_ms = indicator
            .as_ref()
            .and_then(|options| options.interval_ms)
            .filter(|interval| *interval > 0)
            .unwrap_or(DEFAULT_INTERVAL_MS);
        self.current_frame = 0;
        self.start();
    }

    /// When the next frame is due.
    pub fn next_frame_deadline(&self) -> Option<Instant> {
        self.next_frame_at
    }

    /// Advance to the next frame once the deadline has passed.
    pub fn tick(&mut self) {
        if !self.is_animating() {
            return;
        }
        if self.shimmer.is_none() {
            self.current_frame = (self.current_frame + 1) % self.frames.len();
        }
        self.update_display();
        self.next_frame_at = Some(Instant::now() + Duration::from_millis(self.frame_interval_ms()));
    }

    /// Whether a render was requested since the last check.
    pub fn take_render_request(&mut self) -> bool {
        std::mem::take(&mut self.render_requested)
    }

    fn restart_animation(&mut self) {
        self.stop();
        if !self.is_animating() {
            return;
        }
        self.next_frame_at = Some(Instant::now() + Duration::from_millis(self.frame_interval_ms()));
    }

    fn update_display(&mut self) {
        if let Some(palette) = self.shimmer {
            let combined = format!("{}{}", self.message_prefix, self.message);
            let message = match palette {
                Some(palette) => shimmer(&combined, palette, self.started_at.elapsed()),
                None => (self.message_color_fn)(&combined),
            };
            let prefix_width = visible_width(&self.message_prefix);
            let message = if prefix_width == 0 {
                self.rendered_message_prefix.clear();
                message
            } else {
                // Column slices omit the trailing reset; each independently
                // placed fragment must restore the foreground after itself.
                self.rendered_message_prefix = format!(
                    "{}\x1b[39m",
                    slice_by_column(&message, 0, prefix_width, false)
                );
                format!(
                    "{}\x1b[39m",
                    slice_by_column(&message, prefix_width, visible_width(&self.message), false)
                )
            };
            self.text.set_text(format!("{message}{}", self.suffix));
            self.render_requested = true;
            return;
        }

        let frame = self
            .frames
            .get(self.current_frame)
            .cloned()
            .unwrap_or_default();
        let rendered_frame = if self.render_indicator_verbatim {
            frame.clone()
        } else {
            (self.spinner_color_fn)(&frame)
        };
        let indicator = if frame.is_empty() {
            String::new()
        } else {
            format!("{rendered_frame} ")
        };
        let message = (self.message_color_fn)(&self.message);
        self.rendered_message_prefix = if self.message_prefix.is_empty() {
            String::new()
        } else {
            (self.message_color_fn)(&self.message_prefix)
        };
        self.text
            .set_text(format!("{indicator}{message}{}", self.suffix));
        self.render_requested = true;
    }
}

impl Component for Loader {
    fn render(&mut self, width: usize) -> Vec<Line> {
        let mut lines = vec![Line::from("")];
        lines.extend(self.text.render(width));
        lines
    }

    fn invalidate(&mut self) {
        self.text.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gutter_prefix_shimmers_without_taking_space_from_the_message() {
        let color: ColorFn = Rc::new(str::to_owned);
        let mut loader = Loader::new(color.clone(), color, "Working...", None);
        loader.set_shimmer(Some(ShimmerPalette {
            base: (220, 220, 220),
            fade: (0, 0, 0),
        }));
        let without_prefix = loader.render(12);
        loader.set_message_prefix("* ");
        let initial_prefix = loader.rendered_message_prefix().to_owned();
        assert_eq!(
            loader.render(12),
            without_prefix,
            "the gutter prefix must not displace or rewrap the message"
        );
        loader.started_at = Instant::now() - Duration::from_millis(625);
        loader.tick();
        assert_ne!(
            loader.rendered_message_prefix(),
            initial_prefix,
            "the shimmer must change the gutter prefix's color too"
        );
        assert_eq!(visible_width(loader.rendered_message_prefix()), 2);
        loader.set_message_prefix("");
        assert!(
            loader.rendered_message_prefix().is_empty(),
            "removing the prefix must release the gutter"
        );
    }
}
