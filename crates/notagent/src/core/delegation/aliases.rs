//! Star names for delegated children.
//! A subagent is identified by a uuid, which is the right thing for continuing
//! one and the wrong thing for showing one: with three children running, three
//! uuids in an approval dialog tell the user nothing about which request
//! belongs to which piece of work.
//! Every name in the pool is a real astronomical object, in its English
//! spelling. That is deliberate rather than incidental: invented Star Trek
//! worlds are trademarked, and a real star name is just as memorable. Several of
//! the entries are the stars those worlds were hung on anyway.
//! The registry is session-scoped, so a name means one child for as long as the
//! user can see it and is free again afterwards. It is never written into a
//! transcript as an identifier — `session_id` remains the thing that names a
//! continuable child, and an alias that outlived its session would promise a
//! continuity it cannot keep.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};

use rand::prelude::IndexedRandom as _;

/// The pool, one name per line. Blank lines are ignored so the file can be
/// grouped for readability.
const STAR_NAMES: &str = include_str!("star_names.txt");

/// The pool as a list, in file order.
pub fn star_names() -> Vec<&'static str> {
    STAR_NAMES
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect()
}

/// Suffix for the nth pass over an exhausted pool: nothing, then `II`, `III`.
/// A numeral rather than prose ("the second Vega"): a name in a status line has
/// to stay a label, since the row is narrow and the user is scanning it.
fn roman_numeral(value: usize) -> String {
    const NUMERALS: [(usize, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut remaining = value;
    let mut rendered = String::new();
    for (amount, numeral) in NUMERALS {
        while remaining >= amount {
            rendered.push_str(numeral);
            remaining -= amount;
        }
    }
    rendered
}

/// A name on its `pass`th time around: `Vega`, then `Vega II`, then `Vega III`.
fn format_alias(name: &str, pass: usize) -> String {
    if pass == 0 {
        return name.to_owned();
    }
    format!("{name} {}", roman_numeral(pass + 1))
}

#[derive(Default)]
struct AliasState {
    in_use: HashSet<String>,
    /// How many times the pool has been exhausted and reset.
    pass: usize,
}

/// Hands out star names and takes them back.
/// Owned by the session rather than by the task tool: a mode switch rebuilds the
/// tool, and a child that outlived the rebuild would otherwise have its name
/// handed to someone else while it was still running.
#[derive(Clone, Default)]
pub struct AliasRegistry {
    state: Arc<Mutex<AliasState>>,
}

impl AliasRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserves a name. `preferred` skips the draw, which is how a continued
    /// child keeps the name the user already saw.
    /// Draws at random rather than in order. A sequential pool would make
    /// `Wolf 359` the first subagent of every session, and a label that is
    /// always the same is one the user stops reading.
    pub fn reserve(&self, preferred: Option<&str>) -> String {
        let names = star_names();
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);

        if let Some(preferred) = preferred {
            let preferred = preferred.to_owned();
            state.in_use.insert(preferred.clone());
            return preferred;
        }

        // An empty pool would be a broken build rather than a runtime state, but
        // returning a usable name beats panicking in the middle of a delegation.
        if names.is_empty() {
            let fallback = format_alias("Subagent", state.pass);
            state.in_use.insert(fallback.clone());
            return fallback;
        }

        let free: Vec<String> = names
            .iter()
            .map(|name| format_alias(name, state.pass))
            .filter(|name| !state.in_use.contains(name))
            .collect();

        let chosen = match free.choose(&mut rand::rng()) {
            Some(chosen) => chosen.clone(),
            None => {
                // Everything on this pass is taken. Start the next one; the
                // suffix keeps the new names distinct from the live ones, so
                // clearing the set cannot produce a collision.
                state.pass += 1;
                state.in_use.clear();
                let pass = state.pass;
                match names.choose(&mut rand::rng()) {
                    Some(name) => format_alias(name, pass),
                    None => format_alias("Subagent", pass),
                }
            }
        };
        state.in_use.insert(chosen.clone());
        chosen
    }

    /// Returns a name to the pool. Releasing one that was never reserved is not
    /// an error: a child that failed to start still runs its cleanup.
    pub fn release(&self, alias: &str) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .in_use
            .remove(alias);
    }

    /// Whether a name is currently held. For tests and for the task store, which
    /// must not show a name as live after its child ended.
    pub fn is_reserved(&self, alias: &str) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .in_use
            .contains(alias)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pool_is_large_enough_and_free_of_duplicates() {
        let names = star_names();
        assert!(
            names.len() >= 60,
            "the pool holds {} names; a long session would wrap it",
            names.len()
        );
        let unique: HashSet<&&str> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "the pool repeats a name");
    }

    /// The one entry the user asked for by name.
    #[test]
    fn wolf_359_is_in_the_pool() {
        assert!(star_names().contains(&"Wolf 359"));
    }

    #[test]
    fn reserves_distinct_names() {
        let registry = AliasRegistry::new();
        let mut seen = HashSet::new();
        for _ in 0..20 {
            let alias = registry.reserve(None);
            assert!(seen.insert(alias), "the registry handed out a name twice");
        }
    }

    #[test]
    fn a_released_name_can_be_handed_out_again() {
        let registry = AliasRegistry::new();
        let alias = registry.reserve(None);
        assert!(registry.is_reserved(&alias));
        registry.release(&alias);
        assert!(!registry.is_reserved(&alias));
    }

    #[test]
    fn a_preference_skips_the_draw() {
        let registry = AliasRegistry::new();
        assert_eq!(registry.reserve(Some("Vega")), "Vega");
        assert!(registry.is_reserved("Vega"));
    }

    #[test]
    fn an_exhausted_pool_starts_a_numbered_pass() {
        let registry = AliasRegistry::new();
        let count = star_names().len();
        for _ in 0..count {
            registry.reserve(None);
        }
        let overflow = registry.reserve(None);
        assert!(
            overflow.ends_with(" II"),
            "expected a second-pass name, got \"{overflow}\""
        );
    }

    #[test]
    fn numbers_the_passes_in_roman() {
        assert_eq!(format_alias("Vega", 0), "Vega");
        assert_eq!(format_alias("Vega", 1), "Vega II");
        assert_eq!(format_alias("Vega", 2), "Vega III");
        assert_eq!(format_alias("Vega", 3), "Vega IV");
    }
}
