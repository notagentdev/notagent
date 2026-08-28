/// Ring buffer for kill/yank operations.
#[derive(Debug, Default, Clone)]
pub struct KillRing {
    ring: Vec<String>,
}

/// Options of [`KillRing::push`].
#[derive(Debug, Clone, Copy)]
pub struct KillRingPushOptions {
    /// When accumulating: prepend (backward deletion) or append (forward deletion).
    pub prepend: bool,
    /// Merge with the most recent entry instead of creating a new one.
    pub accumulate: bool,
}

impl KillRing {
    /// Empty ring.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add killed text to the ring.
    pub fn push(&mut self, text: &str, options: KillRingPushOptions) {
        if text.is_empty() {
            return;
        }
        if options.accumulate
            && let Some(last) = self.ring.pop()
        {
            self.ring.push(if options.prepend {
                format!("{text}{last}")
            } else {
                format!("{last}{text}")
            });
            return;
        }
        self.ring.push(text.to_string());
    }

    /// Most recent entry without modifying the ring.
    pub fn peek(&self) -> Option<&str> {
        self.ring.last().map(String::as_str)
    }

    /// Move the last entry to the front (yank-pop cycling).
    pub fn rotate(&mut self) {
        if self.ring.len() > 1
            && let Some(last) = self.ring.pop()
        {
            self.ring.insert(0, last);
        }
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.ring.len()
    }

    /// Whether the ring is empty.
    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }
}
