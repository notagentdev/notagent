//! Measure the steady cost of finalized transcript rows in both renderers.
//! Run explicitly with `cargo test -p notagent-tui --release --test transcript_scale -- --ignored --nocapture`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use async_trait::async_trait;
use notagent_tui::components::scroll_view::{ScrollView, ScrollViewOptions};
use notagent_tui::terminal::{InputHandler, ResizeHandler, Terminal};
use notagent_tui::transcript_container::TranscriptContainer;
use notagent_tui::tui::{Component, Container, Line, component_ref};
use notagent_tui::tui_alt_screen::{TuiAltScreen, TuiAltScreenOptions};
use notagent_tui::tui_main_screen::TuiMainScreen;

const WIDTH: usize = 80;
const HEIGHT: usize = 40;
const FRAMES: usize = 50;

thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct CountingAllocator;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

// The allocator counts only the test thread; the workspace benchmark stays
// comparable even when the Rust test harness runs other tests concurrently.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let result = unsafe { System.realloc(pointer, layout, size) };
        if !result.is_null() {
            let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
        }
        result
    }
}

struct SinkTerminal {
    bytes: Rc<Cell<usize>>,
}

#[async_trait(?Send)]
impl Terminal for SinkTerminal {
    fn start(&mut self, _: InputHandler, _: ResizeHandler) {}
    fn stop(&mut self) {}
    async fn drain_input(&mut self, _: Option<u64>, _: Option<u64>) {}
    fn write(&mut self, data: &str) {
        self.bytes.set(self.bytes.get() + data.len());
    }
    fn columns(&self) -> usize {
        WIDTH
    }
    fn rows(&self) -> usize {
        HEIGHT
    }
    fn kitty_protocol_active(&self) -> bool {
        false
    }
    fn move_by(&mut self, _: isize) {}
    fn hide_cursor(&mut self) {}
    fn show_cursor(&mut self) {}
    fn clear_line(&mut self) {}
    fn clear_from_cursor(&mut self) {}
    fn clear_screen(&mut self) {}
    fn set_title(&mut self, _: &str) {}
    fn set_progress(&mut self, _: bool) {}
}

struct CountedRow {
    line: Line,
    calls: Rc<Cell<usize>>,
}

impl Component for CountedRow {
    fn render(&mut self, _: usize) -> Vec<Line> {
        self.calls.set(self.calls.get() + 1);
        vec![self.line.clone()]
    }

    fn invalidate(&mut self) {}
}

fn measure(mut render: impl FnMut(), calls: &Cell<usize>, bytes: &Cell<usize>, label: &str) {
    let before_calls = calls.get();
    let before_bytes = bytes.get();
    let before_allocations = ALLOCATIONS.with(Cell::get);
    let mut durations = Vec::with_capacity(FRAMES);
    for _ in 0..FRAMES {
        let started = Instant::now();
        render();
        durations.push(started.elapsed().as_secs_f64() * 1_000_000.0);
    }
    let allocations = ALLOCATIONS.with(Cell::get) - before_allocations;
    durations.sort_by(f64::total_cmp);
    let median = durations[FRAMES / 2];
    let p95 = durations[(FRAMES * 95).div_ceil(100) - 1];
    println!(
        "{label} median_us={median:.1} p95_us={p95:.1} calls/frame={} allocs/frame={} bytes/frame={}",
        (calls.get() - before_calls) / FRAMES,
        allocations / FRAMES,
        (bytes.get() - before_bytes) / FRAMES,
    );
}

fn transcript(rows: usize, calls: &Rc<Cell<usize>>) -> Container {
    let mut transcript = Container::new();
    for row in 0..rows {
        transcript.add_child(component_ref(CountedRow {
            line: Line::from(format!("history row {row}")),
            calls: Rc::clone(calls),
        }));
    }
    transcript.add_child(component_ref(CountedRow {
        line: Line::from("active row"),
        calls: Rc::clone(calls),
    }));
    transcript
}

fn bounded_transcript(rows: usize, calls: &Rc<Cell<usize>>) -> TranscriptContainer {
    let mut transcript = TranscriptContainer::new();
    transcript.set_regular(true);
    for row in 0..rows {
        transcript.add_child(component_ref(CountedRow {
            line: Line::from(format!("history row {row}")),
            calls: Rc::clone(calls),
        }));
    }
    let active = component_ref(CountedRow {
        line: Line::from("active row"),
        calls: Rc::clone(calls),
    });
    transcript.add_child(Rc::clone(&active));
    transcript.mark_mutable(&active);
    transcript
}

#[test]
#[ignore = "release timing harness"]
fn regular_frames_with_retired_history() {
    for rows in [100, 10_000, 100_000] {
        let calls = Rc::new(Cell::new(0));
        let bytes = Rc::new(Cell::new(0));
        let mut tui = TuiMainScreen::new(Box::new(SinkTerminal {
            bytes: Rc::clone(&bytes),
        }));
        tui.core()
            .add_child(component_ref(bounded_transcript(rows, &calls)));
        tui.render_now(false);
        measure(
            || tui.render_now(false),
            &calls,
            &bytes,
            &format!("bounded regular rows={rows}"),
        );
    }
}

#[test]
#[ignore = "release timing harness"]
fn regular_frames_scale_with_finalized_history() {
    for rows in [100, 10_000, 100_000] {
        let calls = Rc::new(Cell::new(0));
        let bytes = Rc::new(Cell::new(0));
        let mut tui = TuiMainScreen::new(Box::new(SinkTerminal {
            bytes: Rc::clone(&bytes),
        }));
        tui.core()
            .add_child(component_ref(transcript(rows, &calls)));
        tui.render_now(false);
        measure(
            || tui.render_now(false),
            &calls,
            &bytes,
            &format!("regular rows={rows}"),
        );
    }
}

#[test]
#[ignore = "release timing harness"]
fn fullscreen_frames_scale_with_finalized_history() {
    for rows in [100, 10_000, 100_000] {
        let calls = Rc::new(Cell::new(0));
        let bytes = Rc::new(Cell::new(0));
        let mut tui = TuiAltScreen::new(
            Box::new(SinkTerminal {
                bytes: Rc::clone(&bytes),
            }),
            TuiAltScreenOptions::default(),
        );
        tui.core()
            .add_child(component_ref(transcript(rows, &calls)));
        tui.start();
        tui.render_now(false);
        measure(
            || tui.render_now(false),
            &calls,
            &bytes,
            &format!("fullscreen rows={rows}"),
        );
    }
}

#[test]
#[ignore = "release timing harness"]
fn indexed_fullscreen_frames_with_finalized_history() {
    for rows in [100, 10_000, 100_000] {
        let calls = Rc::new(Cell::new(0));
        let mut transcript = bounded_transcript(rows, &calls);
        transcript.set_regular(false);
        let transcript = component_ref(transcript);
        let root = component_ref(ScrollView::new(
            transcript,
            ScrollViewOptions {
                follow_end: true,
                primary: true,
                ..ScrollViewOptions::default()
            },
        ));
        let bytes = Rc::new(Cell::new(0));
        let mut tui = TuiAltScreen::new(
            Box::new(SinkTerminal {
                bytes: Rc::clone(&bytes),
            }),
            TuiAltScreenOptions::default(),
        );
        tui.set_layout_root(Some(root));
        tui.start();
        tui.render_now(false);
        measure(
            || tui.render_now(false),
            &calls,
            &bytes,
            &format!("indexed fullscreen rows={rows}"),
        );
    }
}
