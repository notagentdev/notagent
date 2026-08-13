//! Generic undo stack.
//!
//! 1:1 port of `packages/tui/src/undo-stack.ts` (28 LOC). TS deep-clones on
//! push via `structuredClone`; Rust takes the snapshot by value, which is the
//! same thing for the plain state structs used here.

/// Stack of state snapshots.
#[derive(Debug, Clone)]
pub struct UndoStack<S> {
    stack: Vec<S>,
}

impl<S> Default for UndoStack<S> {
    fn default() -> Self {
        Self { stack: Vec::new() }
    }
}

impl<S> UndoStack<S> {
    /// Empty stack.
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a snapshot.
    pub fn push(&mut self, state: S) {
        self.stack.push(state);
    }

    /// Pop the most recent snapshot.
    pub fn pop(&mut self) -> Option<S> {
        self.stack.pop()
    }

    /// Remove all snapshots.
    pub fn clear(&mut self) {
        self.stack.clear();
    }

    /// Number of snapshots.
    pub fn len(&self) -> usize {
        self.stack.len()
    }

    /// Whether the stack is empty.
    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }
}
