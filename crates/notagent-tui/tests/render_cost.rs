//! Repaint cost of an unchanged transcript.
//! Taken from `../notagent-main-rust/crates/notagent_tui/tests/render_cost.rs`.
//! Deliberately names no line type, so the same file compiles against both the
//! owned and the shared representation and the two can be compared directly.
//! `cargo test -p notagent-tui --release --test render_cost -- --ignored --nocapture`
//! The second case measures no time but the share of lines that could come from
//! a component with a cache. That is the ceiling for the gain from settling the
//! diff by pointer identity: lines rebuilt every frame gain nothing there and
//! cost the pointer comparison on top.
//! Baseline (steps 1 and 2 applied, owned lines, release, Apple M series):
//! ```text
//!   50 blocks,   150 lines:    10.5 us/frame ( 0.07 us/line)
//!  200 blocks,   600 lines:    29.5 us/frame ( 0.05 us/line)
//!  800 blocks,  2400 lines:   112.5 us/frame ( 0.05 us/line)
//!  600 of 600 lines unchanged across two frames (100.0 %)
//! ```
//! Read the numbers as an order of magnitude, not as an assertion; the
//! comparison run after the change belongs beside them.
//! Threshold for that comparison, fixed in advance: if the time per frame at 800
//! blocks stays above 90 us — less than a fifth faster than here — the type
//! change does not pay for itself and steps 5 to 7 are to be taken back. The
//! 100 % above is the ceiling of what is possible: every line of this transcript
//! could be settled by pointer identity.
//! After step 5 (shared lines, cache hits are refcount bumps; the diff still
//! compares by content):
//! ```text
//!   50 blocks,   150 lines:     4.2 us/frame ( 0.03 us/line)
//!  200 blocks,   600 lines:    14.1 us/frame ( 0.02 us/line)
//!  800 blocks,  2400 lines:    55.0 us/frame ( 0.02 us/line)
//! ```
//! 55 us is well below the 90 us threshold; the type change carries itself
//! before the pointer-identity diff (step 7) lands.
//! After steps 7 and 8 the numbers are unchanged (55.0 us/frame at 800
//! blocks), as they must be: this harness measures `render()` only, and those
//! steps act in the paint path — the screen diff settles an unchanged shared
//! line by pointer identity, and the reset pass skips finished markdown lines
//! entirely. The 100 % ceiling above is what step 7 harvests there.

use notagent_tui::Text;
use notagent_tui::tui::{Component, Container, component_ref};

const WIDTH: usize = 111;

fn transcript(blocks: usize) -> Container {
    let mut root = Container::new();
    for i in 0..blocks {
        root.add_child(component_ref(Text::new(
            format!("Line {i}: a piece of transcript that will not change again."),
            1,
            0,
        )));
        root.add_child(component_ref(Text::new(
            format!(
                "  Block {i}: some running text that fills two or three lines and then stays \
                 unchanged, the way finished transcript blocks do."
            ),
            1,
            0,
        )));
    }
    root
}

#[test]
#[ignore = "timing harness, run explicitly"]
fn repaint_of_an_unchanged_transcript() {
    for blocks in [50usize, 200, 800] {
        let mut root = transcript(blocks);

        // Prime every cache; only the steady state is of interest.
        let lines = root.render(WIDTH).len();

        let started = std::time::Instant::now();
        let frames = 200;
        for _ in 0..frames {
            std::hint::black_box(root.render(WIDTH));
        }
        let per_frame = started.elapsed() / frames;
        println!(
            "{blocks:4} blocks, {lines:5} lines: {:7.1} us/frame ({:5.2} us/line)",
            per_frame.as_secs_f64() * 1e6,
            per_frame.as_secs_f64() * 1e6 / lines as f64,
        );
    }
}

#[test]
#[ignore = "survey, run explicitly"]
fn share_of_lines_that_a_cache_could_serve() {
    // Two consecutive repaints with no content change. A line equal in both
    // could come from a cache; one that differs cannot. This is the ceiling, not
    // the actual share — only the pointer comparison itself gives that.
    let mut root = transcript(200);
    let first = root.render(WIDTH);
    let second = root.render(WIDTH);

    let stable = first
        .iter()
        .zip(second.iter())
        .filter(|(a, b)| a == b)
        .count();
    println!(
        "{stable} of {} lines unchanged across two frames ({:.1} %)",
        first.len(),
        stable as f64 * 100.0 / first.len() as f64
    );
}
