//! Spinner with a message.
//!
//! 1:1 port of `packages/tui/src/components/loader.ts` (92 LOC). The animation
//! interval becomes a deadline the caller drives (deviation class 1); TS extends
//! `Text`, the Rust port owns one.

use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::components::shimmer::{SHIMMER_FRAME_MS, ShimmerPalette, shimmer};
use crate::components::text::Text;
use crate::tui::{Component, Line};

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
    render_requested: bool,
    /// When set, the message carries the animation itself and no spinner is
    /// drawn. `None` inside it means the terminal cannot show the fade, in
    /// which case the message is simply left still.
    shimmer: Option<Option<ShimmerPalette>>,
    started_at: Instant,
}

impl Loader {
    /// New loader (TS default message: `"Loading..."`).
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
            render_requested: false,
            shimmer: None,
            started_at: Instant::now(),
        };
        loader.set_indicator(indicator);
        loader
    }

    /// Animate the message itself instead of drawing a spinner beside it.
    ///
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
            let message = match palette {
                Some(palette) => shimmer(&self.message, palette, self.started_at.elapsed()),
                None => (self.message_color_fn)(&self.message),
            };
            self.text.set_text(message);
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
        self.text.set_text(format!("{indicator}{message}"));
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
