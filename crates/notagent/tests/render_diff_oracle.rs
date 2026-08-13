//! Pins `renderDiff` against the TypeScript component.
//!
//! `tools/gen-render-diff-oracle.mjs` renders 20 diff texts with the dark theme
//! in truecolor and writes `tests/fixtures/render-diff-oracle.json`. It is run
//! with `FORCE_COLOR=1` so that chalk emits its sequences: the port always
//! styles, because it replaces chalk with direct ANSI sequences and does not
//! reproduce chalk's TTY colour-level gating (see `crates/notagent/PARITY.md`).

use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::modes::interactive::components::diff::{RenderDiffOptions, render_diff};
use notagent::modes::interactive::theme::theme::init_theme;
use notagent_tui::{TerminalCapabilities, reset_capabilities_cache, set_capabilities};

fn global_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

#[derive(serde::Deserialize)]
struct OracleCase {
    diff: String,
    rendered: String,
}

#[test]
fn matches_the_typescript_component_for_every_oracle_case() {
    let _guard = global_lock();
    set_capabilities(TerminalCapabilities {
        images: None,
        true_color: true,
        hyperlinks: false,
    });
    init_theme(Some("dark"), false);

    let oracle: Vec<OracleCase> =
        serde_json::from_str(include_str!("fixtures/render-diff-oracle.json"))
            .expect("oracle parses");
    assert_eq!(oracle.len(), 20);

    let options = RenderDiffOptions::default();
    for case in &oracle {
        assert_eq!(
            render_diff(&case.diff, &options),
            case.rendered,
            "diff {:?}",
            case.diff
        );
    }
    reset_capabilities_cache();
}
