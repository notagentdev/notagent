use std::time::{Duration, Instant};

/// One elapsed second of the countdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CountdownTick {
    /// Seconds left after this tick (`onTick`).
    pub remaining_seconds: i64,
    /// Whether the countdown ran out with this tick (`onExpire`).
    pub expired: bool,
}

/// Counts whole seconds down to zero.
pub struct CountdownTimer {
    remaining_seconds: i64,
    next_tick_at: Option<Instant>,
}

/// `setInterval(..., 1000)`.
const TICK_INTERVAL: Duration = Duration::from_secs(1);

impl CountdownTimer {
    /// Start the countdown. The caller applies [`CountdownTimer::remaining_seconds`]
    pub fn new(timeout_ms: u64) -> Self {
        Self {
            remaining_seconds: (timeout_ms as f64 / 1000.0).ceil() as i64,
            next_tick_at: Some(Instant::now() + TICK_INTERVAL),
        }
    }

    /// Seconds left.
    pub fn remaining_seconds(&self) -> i64 {
        self.remaining_seconds
    }

    /// When the next tick is due, or `None` once disposed.
    pub fn deadline(&self) -> Option<Instant> {
        self.next_tick_at
    }

    /// Advance the countdown if a tick is due.
    pub fn tick(&mut self) -> Option<CountdownTick> {
        let next_tick_at = self.next_tick_at?;
        if Instant::now() < next_tick_at {
            return None;
        }
        self.remaining_seconds -= 1;
        self.next_tick_at = Some(next_tick_at + TICK_INTERVAL);

        let expired = self.remaining_seconds <= 0;
        if expired {
            self.dispose();
        }
        Some(CountdownTick {
            remaining_seconds: self.remaining_seconds,
            expired,
        })
    }

    /// Stop the countdown (`clearInterval`).
    pub fn dispose(&mut self) {
        self.next_tick_at = None;
    }
}
