//! Transient inverse-video messages for the alternate screen.
//!
//! 1:1 port of `packages/tui/src/components/alt-screen-flash.ts` (51 LOC). The
//! per-message timer becomes a deadline the renderer drives (deviation class 1).

use std::time::{Duration, Instant};

use crate::tui::Component;
use crate::utils::truncate_to_width_opts;

const DEFAULT_DURATION_MS: u64 = 1000;

struct FlashEntry {
    message: String,
    expires_at: Instant,
}

/// Holds transient messages until they expire.
#[derive(Default)]
pub struct AltScreenFlashContainer {
    entries: Vec<FlashEntry>,
    render_requested: bool,
}

impl AltScreenFlashContainer {
    /// Empty container.
    pub fn new() -> Self {
        Self::default()
    }

    /// Show `message` for `duration_ms` (TS default: 1000 ms).
    pub fn flash(&mut self, message: impl Into<String>, duration_ms: Option<u64>) {
        let duration = Duration::from_millis(duration_ms.unwrap_or(DEFAULT_DURATION_MS));
        self.entries.push(FlashEntry {
            message: message.into(),
            expires_at: Instant::now() + duration,
        });
        self.render_requested = true;
    }

    /// When the next message expires.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.entries.iter().map(|entry| entry.expires_at).min()
    }

    /// Drop expired messages; returns `true` when something changed.
    pub fn expire(&mut self) -> bool {
        let now = Instant::now();
        let before = self.entries.len();
        self.entries.retain(|entry| entry.expires_at > now);
        let changed = self.entries.len() != before;
        if changed {
            self.render_requested = true;
        }
        changed
    }

    /// Whether a render was requested since the last check.
    pub fn take_render_request(&mut self) -> bool {
        std::mem::take(&mut self.render_requested)
    }

    /// Drop all messages.
    pub fn dispose(&mut self) {
        self.entries.clear();
    }
}

impl Component for AltScreenFlashContainer {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.entries
            .iter()
            .map(|entry| {
                let message =
                    truncate_to_width_opts(&format!(" {} ", entry.message), width, "", false);
                format!("\x1b[7m{message}\x1b[27m")
            })
            .collect()
    }

    fn invalidate(&mut self) {}
}
