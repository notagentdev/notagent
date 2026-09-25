use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use notagent_tui::components::scroll_view::{ScrollToOptions, ScrollView, ScrollViewOptions};
use notagent_tui::layout::render_layout_frame;
use notagent_tui::terminal::Terminal;
use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::transcript_container::{
    REGULAR_REPLAY_ROWS, TranscriptContainer, regular_replay_rows_from,
};
use notagent_tui::tui::{Component, ComponentRef, Container, Line, component_ref};
use notagent_tui::tui_main_screen::TuiMainScreen;

struct Row(&'static str);

/// A streaming entry: rows in `committed` may enter history before the entry
/// settles, `lines` is how it renders.
struct StreamingRows {
    lines: Rc<RefCell<Vec<String>>>,
    committed: Rc<Cell<usize>>,
    finished: Rc<Cell<bool>>,
    emitted: usize,
}

impl Component for StreamingRows {
    fn render(&mut self, _width: usize) -> Vec<Line> {
        let skip = if self.finished.get() { 0 } else { self.emitted };
        self.lines
            .borrow()
            .iter()
            .skip(skip)
            .cloned()
            .map(Line::from)
            .collect()
    }

    fn invalidate(&mut self) {}

    fn take_stream_history(&mut self, _width: usize) -> Vec<Line> {
        if self.finished.get() {
            return Vec::new();
        }
        let committed = self.committed.get();
        let rows = self
            .lines
            .borrow()
            .iter()
            .take(committed)
            .skip(self.emitted)
            .cloned()
            .map(Line::from)
            .collect();
        self.emitted = self.emitted.max(committed);
        rows
    }

    fn prepare_reflow(&mut self, _width: usize) {
        self.emitted = 0;
    }
}

struct StreamFixture {
    lines: Rc<RefCell<Vec<String>>>,
    committed: Rc<Cell<usize>>,
    finished: Rc<Cell<bool>>,
    entry: ComponentRef,
}

fn stream_fixture(rows: &[&str]) -> StreamFixture {
    let lines = Rc::new(RefCell::new(
        rows.iter().map(|row| (*row).to_string()).collect(),
    ));
    let committed = Rc::new(Cell::new(0));
    let finished = Rc::new(Cell::new(false));
    let entry = component_ref(StreamingRows {
        lines: Rc::clone(&lines),
        committed: Rc::clone(&committed),
        finished: Rc::clone(&finished),
        emitted: 0,
    });
    StreamFixture {
        lines,
        committed,
        finished,
        entry,
    }
}

impl Component for Row {
    fn render(&mut self, _width: usize) -> Vec<Line> {
        vec![Line::from(self.0)]
    }

    fn invalidate(&mut self) {}
}

struct EmptyRow;

impl Component for EmptyRow {
    fn render(&mut self, _width: usize) -> Vec<Line> {
        Vec::new()
    }

    fn invalidate(&mut self) {}
}

struct CountedRow {
    text: String,
    calls: Rc<Cell<usize>>,
}

struct SharedRow(Rc<RefCell<String>>);

impl Component for SharedRow {
    fn render(&mut self, _width: usize) -> Vec<Line> {
        vec![Line::from(self.0.borrow().clone())]
    }

    fn invalidate(&mut self) {}
}

struct SharedLines(Rc<RefCell<Vec<String>>>);

impl Component for SharedLines {
    fn render(&mut self, _width: usize) -> Vec<Line> {
        self.0.borrow().iter().cloned().map(Line::from).collect()
    }

    fn invalidate(&mut self) {}
}

impl Component for CountedRow {
    fn render(&mut self, _width: usize) -> Vec<Line> {
        self.calls.set(self.calls.get() + 1);
        vec![Line::from(self.text.clone())]
    }

    fn invalidate(&mut self) {}
}

#[test]
fn short_regular_transcript_keeps_the_dock_immediately_after_content() {
    let terminal = VirtualTerminal::new(30, 8);
    let mut screen = TuiMainScreen::new(Box::new(terminal.clone()));
    let transcript = Rc::new(RefCell::new(TranscriptContainer::new()));
    transcript.borrow_mut().set_regular(true);
    transcript
        .borrow_mut()
        .add_child(component_ref(Row("message")));
    let mut document = Container::new();
    document.add_child(Rc::clone(&transcript) as ComponentRef);
    screen.core().add_child(component_ref(document));
    screen.core().add_child(component_ref(Row("input")));

    screen.render_now(false);
    let viewport = terminal.get_viewport();
    assert_eq!(viewport[0], "message", "content moved: {viewport:?}");
    assert_eq!(
        viewport[1], "input",
        "input must follow content: {viewport:?}"
    );
    assert!(viewport[2..].iter().all(String::is_empty));

    let reply = component_ref(Row("reply"));
    transcript.borrow_mut().add_child(Rc::clone(&reply));
    screen.render_now(false);
    let viewport = terminal.get_viewport();
    assert_eq!(
        viewport[2], "input",
        "input must follow new content: {viewport:?}"
    );

    transcript.borrow_mut().remove_child(&reply);
    screen.render_now(false);
    let viewport = terminal.get_viewport();
    assert_eq!(
        viewport[1], "input",
        "input must move back up: {viewport:?}"
    );
    assert!(
        viewport[2].is_empty(),
        "old input row remains: {viewport:?}"
    );
}

#[test]
fn first_regular_frame_preserves_existing_terminal_scrollback() {
    let mut terminal = VirtualTerminal::new(30, 3);
    terminal.write("shell one\r\nshell two\r\nshell three\r\nshell four\r\n");
    let mut screen = TuiMainScreen::new(Box::new(terminal.clone()));
    let transcript = Rc::new(RefCell::new(TranscriptContainer::new()));
    transcript.borrow_mut().set_regular(true);
    transcript
        .borrow_mut()
        .add_child(component_ref(Row("message")));
    screen.core().add_child(transcript as ComponentRef);

    screen.render_now(false);
    let buffer = terminal.get_scroll_buffer().join("\n");
    assert!(
        buffer.contains("shell one"),
        "existing scrollback was erased: {buffer:?}"
    );
}

#[test]
fn the_first_regular_frame_keeps_visible_shell_output_in_scrollback() {
    let mut terminal = VirtualTerminal::new(30, 5);
    terminal.write("shell one\r\nshell two\r\n");
    let mut screen = TuiMainScreen::new(Box::new(terminal.clone()));
    let transcript = Rc::new(RefCell::new(TranscriptContainer::new()));
    transcript.borrow_mut().set_regular(true);
    transcript
        .borrow_mut()
        .add_child(component_ref(Row("message")));
    screen.core().add_child(transcript as ComponentRef);

    screen.render_now(false);
    let buffer = terminal.get_scroll_buffer().join("\n");
    for row in ["shell one", "shell two"] {
        assert_eq!(
            buffer.matches(row).count(),
            1,
            "visible shell output must survive the first frame once: {buffer:?}"
        );
    }
    assert_eq!(terminal.get_viewport()[0], "message");
}

#[test]
fn a_forced_redraw_replays_history_without_duplicating_scrollback() {
    let terminal = VirtualTerminal::new(20, 3);
    let mut screen = TuiMainScreen::new(Box::new(terminal.clone()));
    let transcript = Rc::new(RefCell::new(TranscriptContainer::new()));
    transcript.borrow_mut().set_regular(true);
    for row in 0..8 {
        transcript.borrow_mut().add_child(component_ref(CountedRow {
            text: format!("row {row}"),
            calls: Rc::new(Cell::new(0)),
        }));
    }
    screen
        .core()
        .add_child(Rc::clone(&transcript) as ComponentRef);
    screen.render_now(false);

    transcript
        .borrow_mut()
        .replay_recent_rows(20, REGULAR_REPLAY_ROWS);
    screen.request_render(true);
    screen.render_now(false);
    let history = terminal.get_scroll_buffer().join("\n");
    for row in 0..8 {
        let label = format!("row {row}");
        assert_eq!(
            history.matches(&label).count(),
            1,
            "a forced redraw lost or duplicated {label}: {history:?}"
        );
    }
}

#[test]
fn a_height_only_resize_keeps_the_transcript_in_scrollback() {
    let terminal = VirtualTerminal::new(20, 5);
    let mut screen = TuiMainScreen::new(Box::new(terminal.clone()));
    screen.set_resize_reflow_debounce(Duration::ZERO);
    let transcript = Rc::new(RefCell::new(TranscriptContainer::new()));
    transcript.borrow_mut().set_regular(true);
    for row in 0..10 {
        transcript.borrow_mut().add_child(component_ref(CountedRow {
            text: format!("row {row}"),
            calls: Rc::new(Cell::new(0)),
        }));
    }
    screen
        .core()
        .add_child(Rc::clone(&transcript) as ComponentRef);
    screen.render_now(false);
    for height in [3, 5] {
        terminal.resize(20, height);
        screen.render_now(false);
        let history = terminal.get_scroll_buffer().join("\n");
        for row in 0..10 {
            let label = format!("row {row}");
            assert_eq!(
                history.matches(&label).count(),
                1,
                "height {height} lost or duplicated {label}: {history:?}"
            );
        }
    }
}

#[test]
fn fullscreen_heights_stay_correct_after_entries_change_in_regular_mode() {
    let lines = Rc::new(RefCell::new(vec![String::from("a")]));
    let growing = component_ref(SharedLines(Rc::clone(&lines)));
    let mut transcript = TranscriptContainer::new();
    transcript.add_child(Rc::clone(&growing));
    transcript.add_child(component_ref(Row("tail")));
    let indexed = transcript
        .windowed_content(20, 0, 0)
        .map(|window| window.height);
    assert_eq!(indexed, Some(2));

    transcript.set_regular(true);
    lines
        .borrow_mut()
        .extend([String::from("b"), String::from("c")]);
    transcript.mark_changed(&growing);
    transcript.history_regions(20, 10);
    transcript.set_regular(false);

    let window = transcript.windowed_content(20, 0, 10);
    let rendered: Option<Vec<String>> = window
        .as_ref()
        .map(|window| window.lines.iter().map(|line| line.to_string()).collect());
    assert_eq!(
        window.map(|window| window.height),
        Some(4),
        "the index must follow a height change made while regular"
    );
    assert_eq!(
        rendered,
        Some(vec!["a".into(), "b".into(), "c".into(), "tail".into()])
    );
}

#[test]
fn settled_rows_pass_into_native_scrollback_without_repainting_or_duplication() {
    let terminal = VirtualTerminal::new(20, 3);
    let mut screen = TuiMainScreen::new(Box::new(terminal.clone()));
    let transcript = Rc::new(RefCell::new(TranscriptContainer::new()));
    transcript.borrow_mut().set_regular(true);
    for row in ["one", "two", "three", "four", "five"] {
        transcript.borrow_mut().add_child(component_ref(Row(row)));
    }
    screen
        .core()
        .add_child(Rc::clone(&transcript) as ComponentRef);

    screen.render_now(false);
    assert_eq!(
        terminal
            .get_scroll_buffer()
            .join("\n")
            .matches("one")
            .count(),
        1,
        "stable rows should enter native history once"
    );
    screen.render_now(false);
    transcript.borrow_mut().add_child(component_ref(Row("six")));
    screen.render_now(false);

    let document = terminal.get_scroll_buffer().join("\n");
    for row in ["one", "two", "three", "four", "five", "six"] {
        assert_eq!(
            document.matches(row).count(),
            1,
            "{row} must appear exactly once in native scrollback: {document:?}"
        );
    }
    transcript.borrow_mut().set_regular(false);
    assert_eq!(
        transcript.borrow_mut().render(20).len(),
        6,
        "fullscreen must retain every source entry"
    );
}

#[test]
fn a_mutable_row_prevents_later_rows_from_retiring_out_of_order() {
    let terminal = VirtualTerminal::new(20, 3);
    let mut screen = TuiMainScreen::new(Box::new(terminal.clone()));
    let transcript = Rc::new(RefCell::new(TranscriptContainer::new()));
    transcript.borrow_mut().set_regular(true);
    let pending = component_ref(Row("pending"));
    transcript.borrow_mut().add_child(Rc::clone(&pending));
    transcript.borrow_mut().mark_mutable(&pending);
    for _ in 0..5 {
        transcript
            .borrow_mut()
            .add_child(component_ref(Row("settled")));
    }
    screen
        .core()
        .add_child(Rc::clone(&transcript) as ComponentRef);
    screen.render_now(false);
    let buffer = terminal.get_scroll_buffer();
    let native_history = &buffer[..buffer.len().saturating_sub(3)];
    assert!(
        !native_history.join("\n").contains("settled"),
        "later rows cannot pass an earlier mutable row: {native_history:?}"
    );
    transcript.borrow_mut().mark_stable(&pending);
    screen.render_now(false);
    let history = terminal.get_scroll_buffer().join("\n");
    assert_eq!(history.matches("pending").count(), 1);
    assert_eq!(history.matches("settled").count(), 5);
}

#[test]
fn hidden_entries_after_a_pending_row_do_not_cost_each_regular_frame() {
    let terminal = VirtualTerminal::new(20, 3);
    let mut screen = TuiMainScreen::new(Box::new(terminal));
    let transcript = Rc::new(RefCell::new(TranscriptContainer::new()));
    transcript.borrow_mut().set_regular(true);
    let pending = component_ref(Row("pending"));
    transcript.borrow_mut().add_child(Rc::clone(&pending));
    transcript.borrow_mut().mark_mutable(&pending);
    screen
        .core()
        .add_child(Rc::clone(&transcript) as ComponentRef);
    screen.render_now(false);
    for _ in 0..10_000 {
        transcript.borrow_mut().add_child(component_ref(EmptyRow));
    }
    transcript
        .borrow_mut()
        .add_child(component_ref(Row("visible")));
    let before = transcript.borrow().render_counters();
    screen.render_now(false);
    let after = transcript.borrow().render_counters();
    assert_eq!(after.stable_entries - before.stable_entries, 0);
    assert_eq!(
        screen.last_compared_rows(),
        2,
        "only the two visible content rows should be compared"
    );
    assert!(
        after.active_entries - before.active_entries <= 2,
        "hidden entries caused a linear viewport traversal: {}",
        after.active_entries - before.active_entries
    );
}

#[test]
fn fullscreen_frames_visit_only_the_indexed_viewport() {
    let calls = Rc::new(Cell::new(0));
    let transcript = Rc::new(RefCell::new(TranscriptContainer::new()));
    for row in 0..10_000 {
        transcript.borrow_mut().add_child(component_ref(CountedRow {
            text: format!("row {row}"),
            calls: Rc::clone(&calls),
        }));
    }
    let root = component_ref(ScrollView::new(
        Rc::clone(&transcript) as ComponentRef,
        ScrollViewOptions {
            follow_end: true,
            primary: true,
            ..ScrollViewOptions::default()
        },
    ));
    let first = render_layout_frame(&root, 30, 5);
    assert_eq!(first.lines.len(), 5);
    assert!(first.lines[0].contains("row 9995"));
    let before = calls.get();
    let before_counters = transcript.borrow().render_counters();
    let next = render_layout_frame(&root, 30, 5);
    let after_counters = transcript.borrow().render_counters();
    assert!(next.lines[4].contains("row 9999"));
    assert!(
        calls.get() - before < 10,
        "unchanged fullscreen frames must not visit the full transcript"
    );
    assert_eq!(
        after_counters.height_updates - before_counters.height_updates,
        0
    );
    assert!(after_counters.fullscreen_entries - before_counters.fullscreen_entries <= 10);
}

#[test]
fn manual_fullscreen_scroll_stays_on_the_same_entry_after_an_earlier_height_change() {
    let transcript = Rc::new(RefCell::new(TranscriptContainer::new()));
    let earlier = Rc::new(RefCell::new(vec![String::from("earlier")]));
    let earlier_row = component_ref(SharedLines(Rc::clone(&earlier)));
    transcript.borrow_mut().add_child(Rc::clone(&earlier_row));
    for row in 1..30 {
        transcript.borrow_mut().add_child(component_ref(CountedRow {
            text: format!("row {row}"),
            calls: Rc::new(Cell::new(0)),
        }));
    }
    let scroll = ScrollView::new(
        Rc::clone(&transcript) as ComponentRef,
        ScrollViewOptions::default(),
    );
    let state = scroll.state();
    let root = component_ref(scroll);
    render_layout_frame(&root, 30, 5);
    state.borrow_mut().scroll_to(
        10,
        ScrollToOptions {
            disable_follow: true,
        },
    );
    let before = render_layout_frame(&root, 30, 5);
    assert!(before.lines[0].contains("row 10"));

    earlier.borrow_mut().extend([
        String::from("extra one"),
        String::from("extra two"),
        String::from("extra three"),
    ]);
    transcript.borrow_mut().mark_changed(&earlier_row);
    let after = render_layout_frame(&root, 30, 5);
    assert!(
        after.lines[0].contains("row 10"),
        "anchor moved: {:?}",
        after.lines
    );
    assert_eq!(state.borrow().scroll_top(), 13);
}

#[test]
fn regular_replay_selects_recent_complete_entries_without_rendering_all_history() {
    let calls = Rc::new(Cell::new(0));
    let mut transcript = TranscriptContainer::new();
    transcript.set_regular(true);
    for row in 0..10_000 {
        transcript.add_child(component_ref(CountedRow {
            text: format!("row {row}"),
            calls: Rc::clone(&calls),
        }));
    }
    transcript.replay_recent_rows(30, 20);
    assert_eq!(transcript.first_regular_entry(), 9_980);
    assert!(
        calls.get() <= 21,
        "replay must inspect only the selected tail"
    );
    let lines = transcript.render(30);
    assert_eq!(lines.len(), 20);
    assert!(lines[0].contains("row 9980"));
    assert!(lines[19].contains("row 9999"));
}

#[test]
fn bounded_terminal_replay_keeps_older_session_rows_in_fullscreen() {
    let terminal = VirtualTerminal::new(30, 5);
    let mut screen = TuiMainScreen::new(Box::new(terminal.clone()));
    let transcript = Rc::new(RefCell::new(TranscriptContainer::new()));
    transcript.borrow_mut().set_regular(true);
    transcript
        .borrow_mut()
        .set_replay_budget(REGULAR_REPLAY_ROWS);
    for row in 0..1_500 {
        transcript.borrow_mut().add_child(component_ref(CountedRow {
            text: format!("row {row}"),
            calls: Rc::new(Cell::new(0)),
        }));
    }
    transcript
        .borrow_mut()
        .replay_recent_rows(30, REGULAR_REPLAY_ROWS);
    screen
        .core()
        .add_child(Rc::clone(&transcript) as ComponentRef);
    screen.render_now(false);
    let native = terminal.get_scroll_buffer().join("\n");
    assert!(
        !native.contains("row 499"),
        "replay exceeded its row budget"
    );
    assert_eq!(native.matches("row 500").count(), 1, "{native:?}");
    assert_eq!(native.matches("row 1499").count(), 1, "{native:?}");

    transcript.borrow_mut().set_regular(false);
    let scroll = ScrollView::new(
        Rc::clone(&transcript) as ComponentRef,
        ScrollViewOptions::default(),
    );
    let state = scroll.state();
    state.borrow_mut().scroll_to(
        0,
        ScrollToOptions {
            disable_follow: true,
        },
    );
    let frame = render_layout_frame(&component_ref(scroll), 30, 5);
    assert!(frame.lines[0].contains("row 0"), "{:?}", frame.lines);
}

#[test]
fn a_single_oversized_entry_replays_only_its_recent_rows() {
    let mut transcript = TranscriptContainer::new();
    transcript.set_regular(true);
    let rows = (0..10_000).map(|row| format!("row {row}")).collect();
    transcript.add_child(component_ref(SharedLines(Rc::new(RefCell::new(rows)))));
    transcript.replay_recent_rows(30, 20);
    assert_eq!(transcript.first_regular_entry(), 0);
    assert_eq!(transcript.first_regular_offset(), 9_980);
    let visible = transcript.render(30);
    assert_eq!(visible.len(), 20);
    assert_eq!(visible[0].as_ref(), "row 9980");
    assert_eq!(visible[19].as_ref(), "row 9999");
}

#[test]
fn replay_does_not_cut_through_a_kitty_image() {
    use notagent_tui::terminal_image::{KittyImageMetadata, register_kitty_image_metadata};

    register_kitty_image_metadata(KittyImageMetadata {
        image_id: 97_501,
        columns: 3,
        rows: 5,
        width_px: 30,
        height_px: 50,
    });
    let image = "\x1b_Ga=T,i=97501,r=5;AAAA\x1b\\";
    let rows = ["before", image, "", "", "", "", "after"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut transcript = TranscriptContainer::new();
    transcript.set_regular(true);
    transcript.add_child(component_ref(SharedLines(Rc::new(RefCell::new(rows)))));
    transcript.replay_recent_rows(30, 3);
    assert_eq!(
        transcript.first_regular_offset(),
        1,
        "replay boundary must move to the image's first row"
    );
    let replay = transcript.render(30);
    assert_eq!(replay.len(), 6);
    assert_eq!(replay[0].as_ref(), image);
}

#[test]
fn native_history_keeps_a_long_mutable_suffix_out_of_scrollback() {
    let terminal = VirtualTerminal::new(30, 5);
    let mut screen = TuiMainScreen::new(Box::new(terminal.clone()));
    let transcript = Rc::new(RefCell::new(TranscriptContainer::new()));
    transcript.borrow_mut().set_regular(true);
    for row in ["stable one", "stable two"] {
        transcript.borrow_mut().add_child(component_ref(Row(row)));
    }
    let mut document = Container::new();
    document.add_child(Rc::clone(&transcript) as ComponentRef);
    screen.core().add_child(component_ref(document));
    screen.core().add_child(component_ref(Row("dock")));
    screen.render_now(false);

    let pending = component_ref(Row("pending mutable"));
    transcript.borrow_mut().add_child(Rc::clone(&pending));
    transcript.borrow_mut().mark_mutable(&pending);
    for _ in 0..100 {
        transcript
            .borrow_mut()
            .add_child(component_ref(Row("later settled")));
    }
    screen.render_now(false);
    assert!(
        !terminal
            .get_scroll_buffer()
            .join("\n")
            .contains("pending mutable"),
        "a mutable row must remain outside native history"
    );
    assert!(terminal.get_viewport().join("\n").contains("dock"));

    transcript.borrow_mut().mark_stable(&pending);
    screen.render_now(false);
    let history = terminal.get_scroll_buffer().join("\n");
    assert_eq!(
        history.matches("stable one").count(),
        1,
        "first row duplicated: {history:?}"
    );
    assert_eq!(
        history.matches("pending mutable").count(),
        1,
        "pending row missing: {history:?}"
    );
    assert_eq!(
        history.matches("later settled").count(),
        100,
        "settled rows lost: {history:?}"
    );
}

#[test]
fn a_late_change_rebuilds_bounded_history_with_its_static_prefix() {
    let terminal = VirtualTerminal::new(30, 5);
    let mut screen = TuiMainScreen::new(Box::new(terminal.clone()));
    let transcript = Rc::new(RefCell::new(TranscriptContainer::new()));
    transcript.borrow_mut().set_regular(true);
    transcript.borrow_mut().set_replay_budget(5);
    let text = Rc::new(RefCell::new(String::from("old result")));
    let row = component_ref(SharedRow(Rc::clone(&text)));
    transcript.borrow_mut().add_child(Rc::clone(&row));
    let notify = transcript.borrow().external_change_sender();
    let key = Rc::as_ptr(&row).cast::<()>() as usize;
    let mut document = Container::new();
    document.add_child(component_ref(Row("header")));
    document.add_child(Rc::clone(&transcript) as ComponentRef);
    screen.core().add_child(component_ref(document));
    screen.render_now(false);

    *text.borrow_mut() = String::from("new result");
    notify(key);
    screen.render_now(false);
    let history = terminal.get_scroll_buffer().join("\n");
    assert!(!history.contains("old result"), "stale row: {history:?}");
    assert_eq!(history.matches("new result").count(), 1);
    assert_eq!(history.matches("header").count(), 1);

    transcript.borrow_mut().remove_child(&row);
    screen.render_now(false);
    let history = terminal.get_scroll_buffer().join("\n");
    assert!(!history.contains("new result"), "removed row: {history:?}");
    assert_eq!(history.matches("header").count(), 1);
}

#[test]
fn terminal_resize_reflows_recent_history_without_duplicate_rows() {
    let terminal = VirtualTerminal::new(20, 5);
    let mut screen = TuiMainScreen::new(Box::new(terminal.clone()));
    screen.set_resize_reflow_debounce(Duration::ZERO);
    let transcript = Rc::new(RefCell::new(TranscriptContainer::new()));
    transcript.borrow_mut().set_regular(true);
    transcript.borrow_mut().set_replay_budget(20);
    for row in 0..10 {
        transcript.borrow_mut().add_child(component_ref(CountedRow {
            text: format!("row {row}"),
            calls: Rc::new(Cell::new(0)),
        }));
    }
    screen
        .core()
        .add_child(Rc::clone(&transcript) as ComponentRef);
    screen.render_now(false);
    for (width, height) in [(10, 3), (30, 7)] {
        terminal.resize(width, height);
        screen.render_now(false);
        let history = terminal.get_scroll_buffer().join("\n");
        for row in 0..10 {
            let label = format!("row {row}");
            assert_eq!(
                history.matches(&label).count(),
                1,
                "resize to {width}x{height} lost or duplicated {label}: {history:?}"
            );
        }
    }
}

fn counted(text: String) -> ComponentRef {
    component_ref(CountedRow {
        text,
        calls: Rc::new(Cell::new(0)),
    })
}

fn stream_screen(
    stream: &StreamFixture,
) -> (
    VirtualTerminal,
    TuiMainScreen,
    Rc<RefCell<TranscriptContainer>>,
) {
    let terminal = VirtualTerminal::new(20, 4);
    let mut screen = TuiMainScreen::new(Box::new(terminal.clone()));
    let transcript = Rc::new(RefCell::new(TranscriptContainer::new()));
    transcript.borrow_mut().set_regular(true);
    for row in 0..3 {
        transcript
            .borrow_mut()
            .add_child(counted(format!("before {row}")));
    }
    transcript.borrow_mut().add_child(Rc::clone(&stream.entry));
    transcript.borrow_mut().mark_mutable(&stream.entry);
    screen
        .core()
        .add_child(Rc::clone(&transcript) as ComponentRef);
    screen.render_now(false);
    (terminal, screen, transcript)
}

fn settle(stream: &StreamFixture, transcript: &Rc<RefCell<TranscriptContainer>>) {
    stream.finished.set(true);
    transcript.borrow_mut().mark_changed(&stream.entry);
    transcript.borrow_mut().mark_stable(&stream.entry);
}

#[test]
fn a_settled_stream_appends_its_rest_without_rebuilding_scrollback() {
    let stream = stream_fixture(&["para one", "para two", "para three", "para four", "tail"]);
    let (terminal, mut screen, transcript) = stream_screen(&stream);
    stream.committed.set(2);
    screen.render_now(false);
    stream.committed.set(4);
    screen.render_now(false);
    let redraws = screen.full_redraws();

    settle(&stream, &transcript);
    screen.render_now(false);
    for row in 0..6 {
        transcript
            .borrow_mut()
            .add_child(counted(format!("after {row}")));
    }
    screen.render_now(false);

    let history = terminal.get_scroll_buffer().join("\n");
    for row in ["para one", "para two", "para three", "para four", "tail"] {
        assert_eq!(
            history.matches(row).count(),
            1,
            "{row} must enter scrollback exactly once: {history:?}"
        );
    }
    assert_eq!(
        screen.full_redraws(),
        redraws,
        "a stream whose rows match its final render must not rebuild scrollback"
    );
}

#[test]
fn a_settled_stream_that_renders_differently_is_replayed_once() {
    let stream = stream_fixture(&["para one", "para two", "tail"]);
    let (terminal, mut screen, transcript) = stream_screen(&stream);
    stream.committed.set(2);
    screen.render_now(false);

    stream.lines.borrow_mut()[0] = String::from("para ONE");
    settle(&stream, &transcript);
    screen.render_now(false);

    let history = terminal.get_scroll_buffer().join("\n");
    assert!(
        !history.contains("para one"),
        "a streamed row the final render replaced must leave scrollback: {history:?}"
    );
    for row in ["para ONE", "para two", "tail", "before 0"] {
        assert_eq!(history.matches(row).count(), 1, "{row}: {history:?}");
    }
}

#[test]
fn the_first_regular_frame_starts_below_the_shell_output() {
    let mut terminal = VirtualTerminal::new(30, 6);
    terminal.write("shell one\r\nshell two\r\n");
    let mut screen = TuiMainScreen::new(Box::new(terminal.clone()));
    let transcript = Rc::new(RefCell::new(TranscriptContainer::new()));
    transcript.borrow_mut().set_regular(true);
    transcript
        .borrow_mut()
        .add_child(component_ref(Row("message")));
    screen.core().add_child(transcript as ComponentRef);

    screen.start();
    screen.render_now(false);
    let viewport = terminal.get_viewport();
    assert_eq!(
        viewport[..3],
        ["shell one", "shell two", "message"],
        "the transcript must start at the reported cursor row: {viewport:?}"
    );
}

#[test]
fn settled_rows_reach_scrollback_without_erasing_the_display() {
    let terminal = VirtualTerminal::new(20, 3);
    let mut screen = TuiMainScreen::new(Box::new(terminal.clone()));
    let transcript = Rc::new(RefCell::new(TranscriptContainer::new()));
    transcript.borrow_mut().set_regular(true);
    for row in 0..5 {
        transcript
            .borrow_mut()
            .add_child(counted(format!("row {row}")));
    }
    screen
        .core()
        .add_child(Rc::clone(&transcript) as ComponentRef);
    screen.render_now(false);

    terminal.clear_writes();
    for row in 5..9 {
        transcript
            .borrow_mut()
            .add_child(counted(format!("row {row}")));
    }
    screen.render_now(false);
    let writes = terminal.get_writes();
    assert!(
        !writes.contains("\x1b[2J"),
        "some terminals move erased rows into scrollback, so new history must \
         scroll in instead: {writes:?}"
    );
    let history = terminal.get_scroll_buffer().join("\n");
    for row in 0..9 {
        let label = format!("row {row}");
        assert_eq!(history.matches(&label).count(), 1, "{label}: {history:?}");
    }
}

#[test]
fn a_resize_rebuilds_history_only_once_the_size_settles() {
    let terminal = VirtualTerminal::new(20, 5);
    let mut screen = TuiMainScreen::new(Box::new(terminal.clone()));
    screen.set_resize_reflow_debounce(Duration::from_millis(40));
    let transcript = Rc::new(RefCell::new(TranscriptContainer::new()));
    transcript.borrow_mut().set_regular(true);
    for row in 0..10 {
        transcript
            .borrow_mut()
            .add_child(counted(format!("row {row}")));
    }
    screen
        .core()
        .add_child(Rc::clone(&transcript) as ComponentRef);
    screen.render_now(false);
    let redraws = screen.full_redraws();

    for width in [10, 12] {
        terminal.resize(width, 5);
        screen.render_now(false);
        assert_eq!(
            screen.full_redraws(),
            redraws,
            "an intermediate size of a drag must not rebuild scrollback"
        );
    }
    std::thread::sleep(Duration::from_millis(60));
    screen.render_now(false);
    assert_eq!(screen.full_redraws(), redraws + 1);
    let history = terminal.get_scroll_buffer().join("\n");
    for row in 0..10 {
        let label = format!("row {row}");
        assert_eq!(history.matches(&label).count(), 1, "{label}: {history:?}");
    }
}

#[test]
fn a_rebuild_or_rewritten_image_row_deletes_the_old_kitty_placement() {
    let image = "\x1b_Ga=T,i=97611,r=2;AAAA\x1b\\";
    let delete = "\x1b_Ga=d,d=I,i=97611,q=2\x1b\\";
    let terminal = VirtualTerminal::new(30, 8);
    let mut screen = TuiMainScreen::new(Box::new(terminal.clone()));
    let transcript = Rc::new(RefCell::new(TranscriptContainer::new()));
    transcript.borrow_mut().set_regular(true);
    let rows = Rc::new(RefCell::new(vec![image.to_string(), String::new()]));
    let entry = component_ref(SharedLines(Rc::clone(&rows)));
    transcript.borrow_mut().add_child(Rc::clone(&entry));
    transcript.borrow_mut().mark_mutable(&entry);
    screen
        .core()
        .add_child(Rc::clone(&transcript) as ComponentRef);
    screen.render_now(false);

    terminal.clear_writes();
    rows.borrow_mut()[0] = String::from("text instead");
    transcript.borrow_mut().mark_changed(&entry);
    screen.render_now(false);
    let writes = terminal.get_writes();
    let deleted = writes
        .find(delete)
        .expect("the replaced image row deletes its image");
    let repainted = writes.find("text instead").expect("row repainted");
    assert!(deleted < repainted, "the delete must precede the repaint");

    rows.borrow_mut()[0] = image.to_string();
    transcript.borrow_mut().mark_changed(&entry);
    screen.render_now(false);
    terminal.clear_writes();
    screen.request_render(true);
    screen.render_now(false);
    let writes = terminal.get_writes();
    let deleted = writes
        .find(delete)
        .expect("a rebuild deletes the visible image");
    let cleared = writes.find("\x1b[3J").expect("rebuild clears scrollback");
    assert!(
        deleted < cleared,
        "images are deleted before the screen is cleared"
    );
}

#[test]
fn only_a_settled_entry_reports_itself_stable() {
    let mut transcript = TranscriptContainer::new();
    let settled = component_ref(Row("settled"));
    let running = component_ref(Row("running"));
    let foreign = component_ref(Row("foreign"));
    transcript.add_child(Rc::clone(&settled));
    transcript.add_child(Rc::clone(&running));
    transcript.mark_mutable(&running);
    assert!(transcript.is_stable(&settled));
    assert!(!transcript.is_stable(&running));
    assert!(
        !transcript.is_stable(&foreign),
        "a component outside the transcript is free to change"
    );
    transcript.mark_stable(&running);
    assert!(transcript.is_stable(&running));
}

#[test]
fn replay_rows_follow_the_detected_terminal_scrollback() {
    let rows = |pairs: &[(&str, &str)]| {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        regular_replay_rows_from(&move |name| {
            pairs
                .iter()
                .find(|(candidate, _)| candidate == name)
                .map(|(_, value)| value.clone())
        })
    };
    assert_eq!(rows(&[("TERM_PROGRAM", "WezTerm")]), 3_500);
    assert_eq!(rows(&[("TERM_PROGRAM", "vscode")]), 1_000);
    assert_eq!(
        rows(&[("TERM_PROGRAM", "tmux"), ("ALACRITTY_SOCKET", "/tmp/a")]),
        10_000,
        "tmux does not name the terminal that holds the scrollback"
    );
    assert_eq!(rows(&[("WT_SESSION", "id")]), 9_001);
    assert_eq!(
        rows(&[("TERM_PROGRAM", "iTerm.app"), ("WT_SESSION", "id")]),
        REGULAR_REPLAY_ROWS,
        "an explicit TERM_PROGRAM masks later probes"
    );
    assert_eq!(rows(&[]), REGULAR_REPLAY_ROWS);
}

#[test]
fn a_blink_frame_in_the_transcript_stays_below_the_shell_output() {
    use notagent_tui::activity::RUNNING_DOT;
    use notagent_tui::components::text::Text;
    use notagent_tui::tui::CURSOR_MARKER;

    let mut terminal = VirtualTerminal::new(30, 10);
    terminal.write("shell one\r\nshell two\r\nshell three\r\n");
    let mut screen = TuiMainScreen::new(Box::new(terminal.clone()));
    let transcript = Rc::new(RefCell::new(TranscriptContainer::new()));
    transcript.borrow_mut().set_regular(true);
    transcript
        .borrow_mut()
        .add_child(component_ref(Row("message")));
    let working = component_ref(Text::new(format!("{RUNNING_DOT} working"), 0, 0));
    transcript
        .borrow_mut()
        .add_child(Rc::clone(&working) as ComponentRef);
    transcript.borrow_mut().mark_mutable(&working);
    screen.core().add_child(transcript as ComponentRef);
    screen.core().add_child(component_ref(Text::new(
        format!("prompt{CURSOR_MARKER}"),
        0,
        0,
    )));
    screen.core().set_activity_animation(true);
    screen.core().set_show_hardware_cursor(true);

    screen.start();
    screen.render_now(false);
    let cursor = terminal.get_cursor_position();
    for _ in 0..3 {
        let core = screen.core().clone();
        core.tick_activity(
            core.activity_deadline()
                .expect("a visible marker drives the clock"),
        );
        screen.render_pending_frame();
        let viewport = terminal.get_viewport();
        assert_eq!(
            viewport[..3],
            ["shell one", "shell two", "shell three"],
            "a blink frame overwrote the shell output: {viewport:?}"
        );
        assert_eq!(viewport[3], "message", "{viewport:?}");
        assert!(viewport[4].trim_end().ends_with("working"), "{viewport:?}");
        assert_eq!(
            terminal.get_cursor_position(),
            cursor,
            "blinking must return the cursor to the input: {viewport:?}"
        );
    }
}

#[test]
fn a_blink_frame_after_the_transcript_scrolled_repaints_the_status_row_not_the_prompt() {
    use notagent_tui::activity::RUNNING_DOT;
    use notagent_tui::components::text::Text;
    use notagent_tui::tui::CURSOR_MARKER;

    let terminal = VirtualTerminal::new(30, 5);
    let mut screen = TuiMainScreen::new(Box::new(terminal.clone()));
    let transcript = Rc::new(RefCell::new(TranscriptContainer::new()));
    transcript.borrow_mut().set_regular(true);
    for row in ["one", "two", "three"] {
        transcript.borrow_mut().add_child(component_ref(Row(row)));
    }
    let working = component_ref(Text::new(format!("{RUNNING_DOT} working"), 0, 0));
    transcript
        .borrow_mut()
        .add_child(Rc::clone(&working) as ComponentRef);
    transcript.borrow_mut().mark_mutable(&working);
    screen
        .core()
        .add_child(Rc::clone(&transcript) as ComponentRef);
    screen.core().add_child(component_ref(Text::new(
        format!("prompt{CURSOR_MARKER}"),
        0,
        0,
    )));
    screen.core().set_activity_animation(true);
    screen.core().set_show_hardware_cursor(true);
    screen.start();
    screen.render_now(false);

    transcript
        .borrow_mut()
        .add_child(component_ref(Row("four")));
    screen.render_now(false);
    let settled = terminal.get_viewport();
    assert!(settled[4].starts_with("prompt"), "{settled:?}");

    for _ in 0..2 {
        let core = screen.core().clone();
        core.tick_activity(
            core.activity_deadline()
                .expect("a visible marker drives the clock"),
        );
        screen.render_pending_frame();
        let viewport = terminal.get_viewport();
        assert!(
            viewport[4].starts_with("prompt"),
            "a blink frame after a scroll overwrote the prompt: {viewport:?}"
        );
        assert_eq!(
            viewport[3], "four",
            "a blink frame after a scroll overwrote the row below the status: {viewport:?}"
        );
        assert!(viewport[2].trim_end().ends_with("working"), "{viewport:?}");
    }
}
