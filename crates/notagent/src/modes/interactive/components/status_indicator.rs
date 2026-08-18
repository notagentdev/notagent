//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/status-indicator.ts` (114 LOC).
//!
//! TypeScript models the four indicators as subclasses of a `Loader` subclass.
//! Rust has no inheritance: the three stateless subclasses become constructors
//! and the retry variant keeps its countdown as an optional field. The
//! observable surface (`kind`, message, `dispose`, rendering) is unchanged.

use std::rc::Rc;
use std::time::Instant;

use notagent_tui::components::loader::{Loader, LoaderIndicatorOptions};
use notagent_tui::tui::{Component, Line};

use crate::modes::interactive::theme::theme::{ThemeColor, theme};

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

/// Spinner plus status message.
pub struct StatusIndicator {
    /// Which activity is reported.
    pub kind: StatusIndicatorKind,
    loader: Loader,
    countdown: Option<CountdownTimer>,
    retry_message: Option<RetryMessage>,
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

fn warning_spinner() -> Rc<dyn Fn(&str) -> String> {
    Rc::new(|spinner: &str| theme().fg(ThemeColor::Warning, spinner))
}

fn muted_message() -> Rc<dyn Fn(&str) -> String> {
    Rc::new(|text: &str| theme().fg(ThemeColor::Muted, text))
}

impl StatusIndicator {
    /// The base constructor of the TypeScript class.
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
        }
    }

    /// `WorkingStatusIndicator`.
    pub fn working(message: impl Into<String>, indicator: Option<LoaderIndicatorOptions>) -> Self {
        Self::new(
            StatusIndicatorKind::Working,
            accent_spinner(),
            muted_message(),
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
        let mut indicator = Self::new(
            StatusIndicatorKind::Retry,
            warning_spinner(),
            muted_message(),
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
        Self::new(
            StatusIndicatorKind::Compaction,
            accent_spinner(),
            muted_message(),
            label,
            None,
        )
    }

    /// `BranchSummaryStatusIndicator`.
    pub fn branch_summary() -> Self {
        Self::new(
            StatusIndicatorKind::BranchSummary,
            accent_spinner(),
            muted_message(),
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
    /// caller must request a render (`tui.requestRender()` in TypeScript).
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
