//! What the session shows while it is busy.
//! The four activities are constructors rather than subclasses, and the retry
//! variant keeps its countdown as an optional field.
//! None of them draws a spinner. A spinner puts the motion beside the words,
//! where it competes with them for attention while saying nothing the words do
//! not; instead the message animates itself, with a band of fading colour
//! travelling through it.

use std::rc::Rc;
use std::time::{Duration, Instant};

use notagent_tui::components::loader::{Loader, LoaderIndicatorOptions};
use notagent_tui::components::shimmer::{ShimmerPalette, fade_toward_background};
use notagent_tui::tui::{Component, Line};

use crate::modes::interactive::theme::theme::{
    ThemeColor, format_elapsed_live, format_elapsed_precise, theme,
};

use super::countdown_timer::CountdownTimer;
use super::keybinding_hints::key_text;

/// Which activity the indicator reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusIndicatorKind {
    /// The agent is working.
    Working,
    /// A provider request is being retried.
    Retry,
    /// The context is being compacted.
    Compaction,
    /// A branch is being summarised.
    BranchSummary,
}

/// Why a compaction was started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactionStatusReason {
    /// The user asked for it.
    Manual,
    /// The context crossed the threshold.
    Threshold,
    /// The context overflowed.
    Overflow,
}

/// A status message that carries its own animation.
pub struct StatusIndicator {
    /// Which activity is reported.
    pub kind: StatusIndicatorKind,
    loader: Loader,
    countdown: Option<CountdownTimer>,
    retry_message: Option<RetryMessage>,
    started: Instant,
    /// The message without its running time, for the indicators that show one.
    timed_message: Option<String>,
    /// The last running time written into the message, so the message is only
    /// rebuilt when the figure actually changes rather than on every frame.
    shown_elapsed: Option<String>,
    settled: bool,
}

struct RetryMessage {
    attempt: u32,
    max_attempts: u32,
}

impl RetryMessage {
    fn text(&self, seconds: i64) -> String {
        format!(
            "Retrying ({}/{}) in {seconds}s... ({} to cancel)",
            self.attempt,
            self.max_attempts,
            key_text("app.interrupt")
        )
    }
}

fn accent_spinner() -> Rc<dyn Fn(&str) -> String> {
    Rc::new(|spinner: &str| theme().fg(ThemeColor::Accent, spinner))
}

fn message_in(color: ThemeColor) -> Rc<dyn Fn(&str) -> String> {
    Rc::new(move |text: &str| theme().fg(color, text))
}

/// The fade the travelling band pulls the message toward.
/// The background rather than the theme's dim colour: dim is a text colour and
/// stops well short of the ground, which left the band shallow — most visibly
/// in the light theme, where dim is a mid grey a long way from the page. The
/// direction comes from the message colour itself, so it reverses with the
/// theme rather than having to be told which one is in use.
fn shimmer_palette(base: ThemeColor) -> Option<ShimmerPalette> {
    let base = theme().get_fg_rgb(base)?;
    Some(ShimmerPalette {
        base,
        fade: fade_toward_background(base),
    })
}

impl StatusIndicator {
    pub fn new(
        kind: StatusIndicatorKind,
        spinner_color_fn: Rc<dyn Fn(&str) -> String>,
        message_color_fn: Rc<dyn Fn(&str) -> String>,
        message: impl Into<String>,
        indicator: Option<LoaderIndicatorOptions>,
    ) -> Self {
        Self {
            kind,
            loader: Loader::new(spinner_color_fn, message_color_fn, message, indicator),
            countdown: None,
            retry_message: None,
            started: Instant::now(),
            timed_message: None,
            shown_elapsed: None,
            settled: false,
        }
    }

    /// The base constructor for an indicator that animates its own message.
    /// A caller that hands in indicator options has asked for a particular
    /// animation and keeps it; everything else drops the spinner.
    fn animated(
        kind: StatusIndicatorKind,
        base: ThemeColor,
        message: impl Into<String>,
        indicator: Option<LoaderIndicatorOptions>,
    ) -> Self {
        let message = message.into();
        let mut status = Self::new(
            kind,
            accent_spinner(),
            message_in(base),
            message.clone(),
            indicator.clone(),
        );
        if indicator.is_none() {
            status.loader.set_shimmer(shimmer_palette(base));
        }
        if matches!(
            kind,
            StatusIndicatorKind::Working | StatusIndicatorKind::Compaction
        ) {
            status.loader.set_message_prefix("* ");
        }
        // A retry already counts, downwards; a second figure beside it would be
        // two clocks disagreeing about what they measure.
        if kind != StatusIndicatorKind::Retry {
            status.timed_message = Some(message);
        }
        status
    }

    /// Update the running time beside the message.
    /// Returns whether the figure changed, which is the only reason to redraw
    /// on account of the clock — the animation redraws on its own schedule.
    /// The clock is a suffix rather than part of the message, so the shimmer
    /// never travels through it: motion on a figure reads as the figure
    /// changing.
    pub fn tick_elapsed(&mut self) -> bool {
        if self.settled || self.timed_message.is_none() {
            return false;
        }
        let elapsed = format_elapsed_live(self.started.elapsed());
        if elapsed == self.shown_elapsed {
            return false;
        }
        let theme = theme();
        self.loader.set_suffix(match elapsed.as_ref() {
            Some(elapsed) => format!(
                " {}{}{}",
                theme.fg(ThemeColor::Dim, "("),
                theme.fg(ThemeColor::Muted, elapsed),
                theme.fg(ThemeColor::Dim, ")")
            ),
            None => String::new(),
        });
        self.shown_elapsed = elapsed;
        true
    }

    /// When the running time is next worth redrawing for.
    pub fn elapsed_deadline(&self) -> Option<Instant> {
        (!self.settled && self.timed_message.is_some())
            .then(|| Instant::now() + Duration::from_millis(250))
    }

    /// Stop, and leave behind what the work took.
    /// The line stays on screen instead of being replaced by blank rows: how
    /// long a turn ran is the one thing about it a reader cannot reconstruct
    /// afterwards, and blanking the row throws it away at the exact moment it
    /// becomes final.
    pub fn settle(&mut self) {
        if self.settled {
            return;
        }
        let elapsed = format_elapsed_precise(self.started.elapsed());
        self.settled = true;
        if let Some(countdown) = self.countdown.as_mut() {
            countdown.dispose();
        }
        self.countdown = None;
        self.loader.set_shimmer(None);
        self.loader
            .set_message_color(Rc::new(|text: &str| theme().fg(ThemeColor::Dim, text)));
        self.loader.set_suffix("");
        self.loader.set_message(format!("Worked for {elapsed}"));
        self.loader.stop();
    }

    /// Whether the indicator has stopped and is only reporting its runtime.
    pub fn is_settled(&self) -> bool {
        self.settled
    }

    /// `WorkingStatusIndicator`.
    pub fn working(message: impl Into<String>, indicator: Option<LoaderIndicatorOptions>) -> Self {
        Self::animated(
            StatusIndicatorKind::Working,
            ThemeColor::Text,
            message,
            indicator,
        )
    }

    /// `RetryStatusIndicator`.
    pub fn retry(attempt: u32, max_attempts: u32, delay_ms: u64) -> Self {
        let retry_message = RetryMessage {
            attempt,
            max_attempts,
        };
        let countdown = CountdownTimer::new(delay_ms);
        // A retry keeps the warning colour: the countdown is the one status
        // where the message alone does not say that something went wrong.
        let mut indicator = Self::animated(
            StatusIndicatorKind::Retry,
            ThemeColor::Warning,
            retry_message.text((delay_ms as f64 / 1000.0).ceil() as i64),
            None,
        );
        indicator.countdown = Some(countdown);
        indicator.retry_message = Some(retry_message);
        indicator
    }

    /// `CompactionStatusIndicator`.
    pub fn compaction(reason: CompactionStatusReason) -> Self {
        let cancel_hint = format!("({} to cancel)", key_text("app.interrupt"));
        let label = if reason == CompactionStatusReason::Manual {
            format!("Compacting context... {cancel_hint}")
        } else {
            format!(
                "{}Auto-compacting... {cancel_hint}",
                if reason == CompactionStatusReason::Overflow {
                    "Context overflow detected, "
                } else {
                    ""
                }
            )
        };
        Self::animated(
            StatusIndicatorKind::Compaction,
            ThemeColor::Text,
            label,
            None,
        )
    }

    /// `BranchSummaryStatusIndicator`.
    pub fn branch_summary() -> Self {
        Self::animated(
            StatusIndicatorKind::BranchSummary,
            ThemeColor::Text,
            format!(
                "Summarizing branch... ({} to cancel)",
                key_text("app.interrupt")
            ),
            None,
        )
    }

    /// The underlying loader (message, indicator, frame deadline).
    pub fn loader_mut(&mut self) -> &mut Loader {
        &mut self.loader
    }

    /// When the retry countdown is next due.
    pub fn countdown_deadline(&self) -> Option<Instant> {
        self.countdown.as_ref().and_then(CountdownTimer::deadline)
    }

    /// Advance the retry countdown; `true` when the message changed and the
    pub fn tick_countdown(&mut self) -> bool {
        let Some(countdown) = self.countdown.as_mut() else {
            return false;
        };
        let Some(tick) = countdown.tick() else {
            return false;
        };
        if let Some(retry_message) = self.retry_message.as_ref() {
            self.loader
                .set_message(retry_message.text(tick.remaining_seconds));
        }
        if tick.expired {
            self.countdown = None;
        }
        true
    }

    /// Stop the animation and the countdown.
    pub fn dispose(&mut self) {
        if let Some(countdown) = self.countdown.as_mut() {
            countdown.dispose();
        }
        self.countdown = None;
        self.loader.stop();
    }
}

impl Component for StatusIndicator {
    fn render(&mut self, width: usize) -> Vec<Line> {
        if matches!(
            self.kind,
            StatusIndicatorKind::Working | StatusIndicatorKind::Compaction
        ) {
            // Reserve the output gutter as well as the loader's own margins.
            if width < 4 {
                return vec![Line::from(""); 2];
            }
            let lines = self.loader.render(width - 1);
            let prefix = self.loader.rendered_message_prefix();
            let mut first_content_line = true;
            return lines
                .into_iter()
                .map(|line| {
                    if line.is_empty() {
                        line
                    } else if first_content_line && !prefix.is_empty() {
                        first_content_line = false;
                        Line::from(format!(
                            "{prefix}{}",
                            line.strip_prefix(' ').unwrap_or(&line)
                        ))
                    } else {
                        Line::from(format!(" {line}"))
                    }
                })
                .collect();
        }
        self.loader.render(width)
    }

    fn invalidate(&mut self) {
        self.loader.invalidate();
    }
}

/// Two blank lines shown while the agent is idle.
pub struct IdleStatus;

impl Component for IdleStatus {
    fn invalidate(&mut self) {
        // No cached state to invalidate.
    }

    fn render(&mut self, width: usize) -> Vec<Line> {
        let empty_line = Line::from(" ".repeat(width));
        vec![empty_line.clone(), empty_line]
    }
}
