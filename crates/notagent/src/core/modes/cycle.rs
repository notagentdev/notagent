//! Port of `packages/coding-agent/src/core/modes/cycle.ts`.
//!
//! The mode ring.
//!
//! Modes are cycled with a single key, so the order must be stable and
//! predictable. Shipped modes lead in a fixed order; user-authored modes follow
//! alphabetically. That way adding a mode never reshuffles the positions a user
//! has built muscle memory for.

use super::{Mode, locale_compare};

/// Shipped modes, in ring order: one scale ordered by how much happens without
/// the user. Plan is the read-only stop, so nothing can happen there at all.
/// Everything else follows alphabetically.
pub const BUILTIN_MODE_ORDER: [&str; 4] = ["plan", "manual", "auto", "yolo"];

/// The mode a session starts in: full access, every tool use confirmed.
pub const DEFAULT_MODE_ID: &str = "manual";

fn rank(id: &str) -> usize {
    BUILTIN_MODE_ORDER
        .iter()
        .position(|builtin| *builtin == id)
        .unwrap_or(BUILTIN_MODE_ORDER.len())
}

pub fn order_modes(modes: &[Mode]) -> Vec<Mode> {
    let mut ordered = modes.to_vec();
    ordered.sort_by(|a, b| {
        rank(&a.id)
            .cmp(&rank(&b.id))
            .then_with(|| locale_compare(&a.id, &b.id))
    });
    ordered
}

/// Next mode in the ring. Falls back to the first mode when the current id is
/// unknown, which happens when a mode directory disappears between switches.
pub fn next_mode_id(modes: &[Mode], current_id: &str) -> Option<String> {
    let ordered = order_modes(modes);
    if ordered.is_empty() {
        return None;
    }
    let Some(index) = ordered.iter().position(|mode| mode.id == current_id) else {
        return ordered.first().map(|mode| mode.id.clone());
    };
    ordered
        .get((index + 1) % ordered.len())
        .map(|mode| mode.id.clone())
}

/// Resolves the id a session should start with, given what is available.
pub fn initial_mode_id(modes: &[Mode]) -> Option<String> {
    let ordered = order_modes(modes);
    ordered
        .iter()
        .find(|mode| mode.id == DEFAULT_MODE_ID)
        .or_else(|| ordered.first())
        .map(|mode| mode.id.clone())
}
