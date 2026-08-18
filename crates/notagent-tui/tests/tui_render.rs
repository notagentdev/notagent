//! Port of `packages/tui/test/tui-render.test.ts` (832 LOC).
//!
//! The seven Kitty image cases need `encodeKitty` and the `Image` component and
//! follow with tasks 9 and 12; everything else is ported here.
//!
//! `terminal.waitForRender()` of the TS suite becomes `tui.wait_for_render()`:
//! the render loop is driven by the caller in Rust, not by a timer callback.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{Component, Line, TuiStopOptions, component_ref, shared_lines};
use notagent_tui::tui_main_screen::TuiMainScreen;

fn guard() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `class TestComponent` — the lines live behind a handle so the test can change
/// them after the component was mounted (TS mutates the object directly).
#[derive(Clone, Default)]
struct TestHandle {
    lines: Rc<RefCell<Vec<String>>>,
    render_count: Rc<Cell<usize>>,
    last_input: Rc<RefCell<Option<String>>>,
}

impl TestHandle {
    fn set_lines<I: IntoIterator<Item = S>, S: Into<String>>(&self, lines: I) {
        *self.lines.borrow_mut() = lines.into_iter().map(Into::into).collect();
    }
}

struct TestComponent {
    handle: TestHandle,
    handles_input: bool,
}

impl Component for TestComponent {
    fn render(&mut self, _width: usize) -> Vec<Line> {
        self.handle
            .render_count
            .set(self.handle.render_count.get() + 1);
        shared_lines(self.handle.lines.borrow().clone())
    }

    fn handle_input(&mut self, data: &str) {
        if self.handles_input {
            *self.handle.last_input.borrow_mut() = Some(data.to_string());
            *self.handle.lines.borrow_mut() = vec![data.to_string()];
        }
    }

    fn invalidate(&mut self) {}
}

fn test_component() -> (TestHandle, notagent_tui::tui::ComponentRef) {
    let handle = TestHandle::default();
    let component = component_ref(TestComponent {
        handle: handle.clone(),
        handles_input: false,
    });
    (handle, component)
}

fn input_component() -> (TestHandle, notagent_tui::tui::ComponentRef) {
    let handle = TestHandle::default();
    let component = component_ref(TestComponent {
        handle: handle.clone(),
        handles_input: true,
    });
    (handle, component)
}

fn numbered_lines(prefix: &str, count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("{prefix} {index}"))
        .collect()
}

/// Set environment variables for the duration of `body`.
fn with_env(vars: &[(&str, Option<&str>)], body: impl FnOnce()) {
    let previous: Vec<(String, Option<String>)> = vars
        .iter()
        .map(|(name, _)| ((*name).to_string(), std::env::var(name).ok()))
        .collect();
    for (name, value) in vars {
        // SAFETY: tests in this binary are serialized through `guard()`.
        unsafe {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
    body();
    for (name, value) in &previous {
        // SAFETY: see above.
        unsafe {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

// describe("TUI render scheduling")

#[tokio::test]
async fn renders_keyboard_input_without_waiting_for_a_throttled_frame() {
    let terminal = VirtualTerminal::new(40, 10);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (handle, component) = input_component();
    handle.set_lines(["initial"]);
    tui.core().add_child(component.clone());
    tui.core().set_focus(Some(component));
    tui.start();
    tui.render_now(false);
    let render_count_before_input = handle.render_count.get();

    // Queue a normal throttled render first; keyboard input must preempt it.
    handle.set_lines(["pending"]);
    tui.request_render(false);
    terminal.send_input("first");
    terminal.send_input("second");
    terminal.send_input("typed");
    tui.wait_for_render().await;

    assert_eq!(handle.render_count.get(), render_count_before_input + 1);
    assert_eq!(handle.lines.borrow().as_slice(), ["typed".to_string()]);
    tui.stop(TuiStopOptions::default());
}

// describe("TUI debug logging")

// The environment is process-global; these tests take the guard and hold it
// across awaits on purpose — every test in this binary runs on its own
// current-thread runtime, so no other task can be blocked by it.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn writes_redraw_logs_to_the_provided_directory() {
    let _guard = guard();
    let log_dir = std::env::temp_dir().join(format!("notagent-tui-log-{}", std::process::id()));
    std::fs::create_dir_all(&log_dir).expect("create log dir");

    with_env(&[("NOTAGENT_DEBUG_REDRAW", Some("1"))], || {
        let terminal = VirtualTerminal::new(40, 10);
        let mut tui =
            TuiMainScreen::with_options(Box::new(terminal.clone()), None, Some(log_dir.clone()));
        let (handle, component) = test_component();
        tui.core().add_child(component);
        handle.set_lines(["test"]);
        tui.start();
        tui.render_now(false);

        let log = std::fs::read_to_string(log_dir.join("notagent-debug.log")).expect("log written");
        assert!(log.contains("fullRender: first render"), "log was: {log:?}");
        tui.stop(TuiStopOptions::default());
    });

    let _ = std::fs::remove_dir_all(&log_dir);
}

// describe("TUI resize handling")

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn triggers_full_re_render_when_terminal_height_changes() {
    let _guard = guard();
    let terminal = VirtualTerminal::new(40, 10);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (handle, component) = test_component();
    tui.core().add_child(component);

    handle.set_lines(["Line 0", "Line 1", "Line 2"]);
    tui.start();
    tui.wait_for_render().await;

    let initial_redraws = tui.full_redraws();

    terminal.resize(40, 15);
    tui.wait_for_render().await;

    assert!(
        tui.full_redraws() > initial_redraws,
        "Height change should trigger full redraw"
    );
    let viewport = terminal.get_viewport();
    assert!(
        viewport[0].contains("Line 0"),
        "Content preserved after height change"
    );

    tui.stop(TuiStopOptions::default());
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn skips_full_re_render_on_height_changes_in_termux() {
    let _guard = guard();
    let terminal = VirtualTerminal::new(40, 10);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (handle, component) = test_component();
    tui.core().add_child(component);

    handle.set_lines(numbered_lines("Line", 20));

    // TERMUX_VERSION is read inside do_render, so set it around the frames.
    let previous = std::env::var("TERMUX_VERSION").ok();
    // SAFETY: tests in this binary are serialized through `guard()`.
    unsafe { std::env::set_var("TERMUX_VERSION", "1") };

    tui.start();
    tui.wait_for_render().await;
    terminal.clear_writes();

    let initial_redraws = tui.full_redraws();
    for height in [15, 8, 14, 11] {
        terminal.resize(40, height);
        tui.request_render(false);
        tui.wait_for_render().await;
    }

    assert_eq!(
        tui.full_redraws(),
        initial_redraws,
        "Height change should not trigger full redraw"
    );
    assert!(
        !terminal.get_writes().contains("\x1b[2J"),
        "Height change should not clear the screen"
    );
    assert!(
        !terminal.get_writes().contains("\x1b[3J"),
        "Height change should not clear scrollback"
    );
    assert!(
        terminal.get_viewport().join("\n").contains("Line 19"),
        "Latest content remains visible after resize"
    );

    tui.stop(TuiStopOptions::default());
    // SAFETY: see above.
    unsafe {
        match previous {
            Some(value) => std::env::set_var("TERMUX_VERSION", value),
            None => std::env::remove_var("TERMUX_VERSION"),
        }
    }
}

#[tokio::test]
async fn triggers_full_re_render_when_terminal_width_changes() {
    let terminal = VirtualTerminal::new(40, 10);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (handle, component) = test_component();
    tui.core().add_child(component);

    handle.set_lines(["Line 0", "Line 1", "Line 2"]);
    tui.start();
    tui.wait_for_render().await;

    let initial_redraws = tui.full_redraws();
    terminal.resize(60, 10);
    tui.wait_for_render().await;

    assert!(
        tui.full_redraws() > initial_redraws,
        "Width change should trigger full redraw"
    );
    tui.stop(TuiStopOptions::default());
}

// describe("TUI content shrinkage")

#[tokio::test]
async fn clears_empty_rows_when_content_shrinks_significantly() {
    let terminal = VirtualTerminal::new(40, 10);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    tui.core().set_clear_on_shrink(true);
    let (handle, component) = test_component();
    tui.core().add_child(component);

    handle.set_lines(["Line 0", "Line 1", "Line 2", "Line 3", "Line 4", "Line 5"]);
    tui.start();
    tui.wait_for_render().await;

    let initial_redraws = tui.full_redraws();

    handle.set_lines(["Line 0", "Line 1"]);
    tui.request_render(false);
    tui.wait_for_render().await;

    assert!(
        tui.full_redraws() > initial_redraws,
        "Content shrinkage should trigger full redraw"
    );
    let viewport = terminal.get_viewport();
    assert!(viewport[0].contains("Line 0"), "First line preserved");
    assert!(viewport[1].contains("Line 1"), "Second line preserved");
    assert_eq!(viewport[2].trim(), "", "Line 2 should be cleared");
    assert_eq!(viewport[3].trim(), "", "Line 3 should be cleared");

    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn handles_shrink_to_single_line() {
    let terminal = VirtualTerminal::new(40, 10);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    tui.core().set_clear_on_shrink(true);
    let (handle, component) = test_component();
    tui.core().add_child(component);

    handle.set_lines(["Line 0", "Line 1", "Line 2", "Line 3"]);
    tui.start();
    tui.wait_for_render().await;

    handle.set_lines(["Only line"]);
    tui.request_render(false);
    tui.wait_for_render().await;

    let viewport = terminal.get_viewport();
    assert!(viewport[0].contains("Only line"), "Single line rendered");
    assert_eq!(viewport[1].trim(), "", "Line 1 should be cleared");

    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn handles_shrink_to_empty() {
    let terminal = VirtualTerminal::new(40, 10);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    tui.core().set_clear_on_shrink(true);
    let (handle, component) = test_component();
    tui.core().add_child(component);

    handle.set_lines(["Line 0", "Line 1", "Line 2"]);
    tui.start();
    tui.wait_for_render().await;

    handle.set_lines(Vec::<String>::new());
    tui.request_render(false);
    tui.wait_for_render().await;

    let viewport = terminal.get_viewport();
    assert_eq!(viewport[0].trim(), "", "Line 0 should be cleared");
    assert_eq!(viewport[1].trim(), "", "Line 1 should be cleared");

    tui.stop(TuiStopOptions::default());
}

// describe("TUI differential rendering")

#[tokio::test]
async fn tracks_cursor_correctly_when_content_shrinks_with_unchanged_remaining_lines() {
    let terminal = VirtualTerminal::new(40, 10);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (handle, component) = test_component();
    tui.core().add_child(component);

    handle.set_lines(["Line 0", "Line 1", "Line 2", "Line 3", "Line 4"]);
    tui.start();
    tui.wait_for_render().await;

    handle.set_lines(["Line 0", "Line 1", "Line 2"]);
    tui.request_render(false);
    tui.wait_for_render().await;

    handle.set_lines(["Line 0", "CHANGED", "Line 2"]);
    tui.request_render(false);
    tui.wait_for_render().await;

    let viewport = terminal.get_viewport();
    assert!(
        viewport[1].contains("CHANGED"),
        "Expected \"CHANGED\" on line 1, got: {}",
        viewport[1]
    );

    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn renders_correctly_when_only_a_middle_line_changes_spinner_case() {
    let terminal = VirtualTerminal::new(40, 10);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (handle, component) = test_component();
    tui.core().add_child(component);

    handle.set_lines(["Header", "Working...", "Footer"]);
    tui.start();
    tui.wait_for_render().await;

    for frame in ["|", "/", "-", "\\"] {
        handle.set_lines([
            "Header".to_string(),
            format!("Working {frame}"),
            "Footer".to_string(),
        ]);
        tui.request_render(false);
        tui.wait_for_render().await;

        let viewport = terminal.get_viewport();
        assert!(
            viewport[0].contains("Header"),
            "Header preserved: {}",
            viewport[0]
        );
        assert!(
            viewport[1].contains(&format!("Working {frame}")),
            "Spinner updated: {}",
            viewport[1]
        );
        assert!(
            viewport[2].contains("Footer"),
            "Footer preserved: {}",
            viewport[2]
        );
    }

    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn renders_correctly_when_first_line_changes_but_rest_stays_same() {
    let terminal = VirtualTerminal::new(40, 10);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (handle, component) = test_component();
    tui.core().add_child(component);

    handle.set_lines(["Line 0", "Line 1", "Line 2", "Line 3"]);
    tui.start();
    tui.wait_for_render().await;

    handle.set_lines(["CHANGED", "Line 1", "Line 2", "Line 3"]);
    tui.request_render(false);
    tui.wait_for_render().await;

    let viewport = terminal.get_viewport();
    assert!(viewport[0].contains("CHANGED"));
    assert!(viewport[1].contains("Line 1"));
    assert!(viewport[2].contains("Line 2"));
    assert!(viewport[3].contains("Line 3"));

    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn renders_correctly_when_last_line_changes_but_rest_stays_same() {
    let terminal = VirtualTerminal::new(40, 10);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (handle, component) = test_component();
    tui.core().add_child(component);

    handle.set_lines(["Line 0", "Line 1", "Line 2", "Line 3"]);
    tui.start();
    tui.wait_for_render().await;

    handle.set_lines(["Line 0", "Line 1", "Line 2", "CHANGED"]);
    tui.request_render(false);
    tui.wait_for_render().await;

    let viewport = terminal.get_viewport();
    assert!(viewport[0].contains("Line 0"));
    assert!(viewport[1].contains("Line 1"));
    assert!(viewport[2].contains("Line 2"));
    assert!(viewport[3].contains("CHANGED"));

    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn renders_correctly_when_multiple_non_adjacent_lines_change() {
    let terminal = VirtualTerminal::new(40, 10);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (handle, component) = test_component();
    tui.core().add_child(component);

    handle.set_lines(["Line 0", "Line 1", "Line 2", "Line 3", "Line 4"]);
    tui.start();
    tui.wait_for_render().await;

    handle.set_lines(["Line 0", "CHANGED 1", "Line 2", "CHANGED 3", "Line 4"]);
    tui.request_render(false);
    tui.wait_for_render().await;

    let viewport = terminal.get_viewport();
    assert!(viewport[0].contains("Line 0"));
    assert!(viewport[1].contains("CHANGED 1"));
    assert!(viewport[2].contains("Line 2"));
    assert!(viewport[3].contains("CHANGED 3"));
    assert!(viewport[4].contains("Line 4"));

    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn handles_transition_from_content_to_empty_and_back_to_content() {
    let terminal = VirtualTerminal::new(40, 10);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (handle, component) = test_component();
    tui.core().add_child(component);

    handle.set_lines(["Line 0", "Line 1", "Line 2"]);
    tui.start();
    tui.wait_for_render().await;
    assert!(terminal.get_viewport()[0].contains("Line 0"));

    handle.set_lines(Vec::<String>::new());
    tui.request_render(false);
    tui.wait_for_render().await;

    handle.set_lines(["New Line 0", "New Line 1"]);
    tui.request_render(false);
    tui.wait_for_render().await;

    let viewport = terminal.get_viewport();
    assert!(viewport[0].contains("New Line 0"), "got: {}", viewport[0]);
    assert!(viewport[1].contains("New Line 1"), "got: {}", viewport[1]);

    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn full_re_renders_when_deleted_lines_move_the_viewport_upward() {
    let terminal = VirtualTerminal::new(20, 5);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (handle, component) = test_component();
    tui.core().add_child(component);

    handle.set_lines(numbered_lines("Line", 12));
    tui.start();
    tui.wait_for_render().await;

    let initial_redraws = tui.full_redraws();

    handle.set_lines(numbered_lines("Line", 7));
    tui.request_render(false);
    tui.wait_for_render().await;

    assert!(
        tui.full_redraws() > initial_redraws,
        "Shrink should trigger a full redraw"
    );
    assert_eq!(
        terminal.get_viewport(),
        ["Line 2", "Line 3", "Line 4", "Line 5", "Line 6"]
    );

    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn appends_after_a_shrink_without_another_full_redraw_once_the_viewport_is_reset() {
    let terminal = VirtualTerminal::new(20, 5);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (handle, component) = test_component();
    tui.core().add_child(component);

    handle.set_lines(numbered_lines("Line", 8));
    tui.start();
    tui.wait_for_render().await;

    let initial_redraws = tui.full_redraws();

    handle.set_lines(["Line 0", "Line 1"]);
    tui.request_render(false);
    tui.wait_for_render().await;

    assert!(
        tui.full_redraws() > initial_redraws,
        "Shrink should reset the viewport with a full redraw"
    );
    let redraws_after_shrink = tui.full_redraws();

    handle.set_lines(["Line 0", "Line 1", "Line 2"]);
    tui.request_render(false);
    tui.wait_for_render().await;

    assert_eq!(
        tui.full_redraws(),
        redraws_after_shrink,
        "Append should stay on the differential path"
    );
    assert_eq!(
        terminal.get_viewport(),
        ["Line 0", "Line 1", "Line 2", "", ""]
    );

    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn clears_stale_content_when_max_lines_rendered_was_inflated_by_a_transient_component() {
    let terminal = VirtualTerminal::new(40, 10);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (chat, chat_component) = test_component();
    let (editor, editor_component) = test_component();
    tui.core().add_child(chat_component);
    tui.core().add_child(editor_component);

    let long_chat = numbered_lines("Chat", 15);
    let short_chat = numbered_lines("Chat", 12);
    let editor_lines = vec![
        "Editor 0".to_string(),
        "Editor 1".to_string(),
        "Editor 2".to_string(),
    ];
    let selector_lines = numbered_lines("Selector", 8);

    chat.set_lines(long_chat);
    editor.set_lines(editor_lines.clone());
    tui.start();
    tui.wait_for_render().await;

    editor.set_lines(selector_lines);
    tui.request_render(false);
    tui.wait_for_render().await;

    editor.set_lines(editor_lines);
    tui.request_render(false);
    tui.wait_for_render().await;

    let redraws_before_switch = tui.full_redraws();
    chat.set_lines(short_chat);
    tui.request_render(false);
    tui.wait_for_render().await;

    assert!(
        tui.full_redraws() > redraws_before_switch,
        "Branch switch should trigger a full redraw"
    );

    let viewport = terminal.get_viewport();
    for (index, line) in viewport.iter().enumerate() {
        for stale in ["Chat 12", "Chat 13", "Chat 14"] {
            assert!(
                !line.contains(stale),
                "Stale {stale:?} at viewport row {index}"
            );
        }
    }
    assert_eq!(
        viewport,
        [
            "Chat 5", "Chat 6", "Chat 7", "Chat 8", "Chat 9", "Chat 10", "Chat 11", "Editor 0",
            "Editor 1", "Editor 2",
        ]
    );

    tui.stop(TuiStopOptions::default());
}

// describe("TUI Kitty image cleanup")

use notagent_tui::components::image::{Image, ImageOptions, ImageTheme};
use notagent_tui::terminal_image::{
    CellDimensions, EncodeKittyOptions, ImageDimensions, ImageProtocol, TerminalCapabilities,
    delete_kitty_image, encode_kitty, reset_capabilities_cache, set_capabilities,
    set_cell_dimensions,
};

/// Pretend to run in a Kitty-capable terminal with 10×10 px cells.
fn with_kitty_terminal(body: impl FnOnce()) {
    set_capabilities(TerminalCapabilities {
        images: Some(ImageProtocol::Kitty),
        true_color: true,
        hyperlinks: true,
    });
    set_cell_dimensions(CellDimensions {
        width_px: 10,
        height_px: 10,
    });
    body();
    reset_capabilities_cache();
    set_cell_dimensions(CellDimensions {
        width_px: 9,
        height_px: 18,
    });
}

fn test_image(max_width_cells: usize, size_px: u32) -> Image {
    Image::new(
        "AAAA",
        "image/png",
        ImageTheme {
            fallback_color: std::rc::Rc::new(|value: &str| value.to_string()),
        },
        ImageOptions {
            max_width_cells: Some(max_width_cells),
            ..ImageOptions::default()
        },
        Some(ImageDimensions {
            width_px: size_px,
            height_px: size_px,
        }),
    )
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn clears_reserved_kitty_image_rows_before_drawing_appended_placements() {
    let _guard = guard();
    let terminal = VirtualTerminal::new(40, 10);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (handle, component) = test_component();
    tui.core().add_child(component);

    let image_lines = with_kitty_image_lines(2, 20);
    handle.set_lines(["before"]);
    tui.start();
    tui.wait_for_render().await;
    terminal.clear_writes();

    let image_sequence = image_lines[0].clone();
    let mut lines = vec!["before".to_string()];
    lines.extend(image_lines);
    lines.push("after".to_string());
    handle.set_lines(lines);
    tui.request_render(false);
    tui.wait_for_render().await;

    let writes = terminal.get_writes();
    assert!(
        writes.contains(&format!("\x1b[2K\r\n\x1b[2K\x1b[1A{image_sequence}\x1b[1B")),
        "reserved rows should be cleared before the image placement is drawn"
    );
    assert!(
        !writes.contains(&format!("{image_sequence}\r\n\x1b[2K")),
        "reserved row clears must not run after the image placement is drawn"
    );

    tui.stop(TuiStopOptions::default());
}

/// Renders the test image inside the Kitty capability scope.
fn with_kitty_image_lines(max_width_cells: usize, size_px: u32) -> Vec<String> {
    let mut lines = Vec::new();
    with_kitty_terminal(|| {
        lines = test_image(max_width_cells, size_px)
            .render(40)
            .iter()
            .map(|line| line.to_string())
            .collect();
    });
    // The renderer itself only needs is_image_line and the registered metadata,
    // both of which survive leaving the capability scope.
    lines
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn deletes_changed_image_ids_before_drawing_moved_placements() {
    let _guard = guard();
    let terminal = VirtualTerminal::new(40, 10);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (handle, component) = test_component();
    tui.core().add_child(component);

    let old_image = encode_kitty(
        "AAAA",
        EncodeKittyOptions {
            columns: Some(2),
            rows: Some(2),
            image_id: Some(42),
            move_cursor: Some(false),
        },
    );
    handle.set_lines(["top".to_string(), old_image]);
    tui.start();
    tui.wait_for_render().await;
    terminal.clear_writes();

    let new_image = encode_kitty(
        "BBBB",
        EncodeKittyOptions {
            columns: Some(2),
            rows: Some(1),
            image_id: Some(42),
            move_cursor: Some(false),
        },
    );
    handle.set_lines([new_image.clone(), String::new()]);
    tui.request_render(false);
    tui.wait_for_render().await;

    let writes = terminal.get_writes();
    let delete_index = writes
        .find(&delete_kitty_image(42))
        .expect("changed old image should be deleted");
    let draw_index = writes.find(&new_image).expect("new image should be drawn");
    assert!(
        delete_index < draw_index,
        "old image must be deleted before the new placement is drawn"
    );

    tui.stop(TuiStopOptions::default());
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn redraws_image_lines_when_an_earlier_reserved_image_row_changes() {
    let _guard = guard();
    let terminal = VirtualTerminal::new(40, 10);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (handle, component) = test_component();
    tui.core().add_child(component);

    let image = encode_kitty(
        "AAAA",
        EncodeKittyOptions {
            columns: Some(2),
            rows: Some(2),
            image_id: Some(88),
            move_cursor: Some(false),
        },
    );
    handle.set_lines([String::new(), image.clone()]);
    tui.start();
    tui.wait_for_render().await;
    terminal.clear_writes();

    handle.set_lines(["covered".to_string(), image.clone()]);
    tui.request_render(false);
    tui.wait_for_render().await;

    let writes = terminal.get_writes();
    let delete_index = writes
        .find(&delete_kitty_image(88))
        .expect("image should be deleted when a reserved row changes");
    let draw_index = writes
        .find(&image)
        .expect("unchanged image line should be redrawn");
    assert!(
        delete_index < draw_index,
        "old placement must be deleted before the image line is redrawn"
    );
    assert!(
        !writes.contains("\x1b[2J"),
        "reserved row changes should not force a full redraw"
    );

    tui.stop(TuiStopOptions::default());
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn deletes_previously_rendered_image_ids_during_full_redraws() {
    let _guard = guard();
    let terminal = VirtualTerminal::new(40, 10);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (handle, component) = test_component();
    tui.core().add_child(component);

    let image = encode_kitty(
        "AAAA",
        EncodeKittyOptions {
            columns: Some(2),
            rows: Some(2),
            image_id: Some(77),
            move_cursor: Some(false),
        },
    );
    handle.set_lines([image]);
    tui.start();
    tui.wait_for_render().await;
    terminal.clear_writes();

    handle.set_lines(["plain text"]);
    tui.request_render(true);
    tui.wait_for_render().await;

    let writes = terminal.get_writes();
    let delete_index = writes
        .find(&delete_kitty_image(77))
        .expect("previous image should be deleted during full redraw");
    let clear_index = writes
        .find("\x1b[2J")
        .expect("full redraw should clear the screen");
    assert!(
        delete_index < clear_index,
        "old image should be deleted before the screen is cleared"
    );

    tui.stop(TuiStopOptions::default());
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn falls_back_to_full_redraw_when_a_kitty_pre_clear_would_scroll() {
    let _guard = guard();
    let terminal = VirtualTerminal::new(40, 2);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (handle, component) = test_component();
    tui.core().add_child(component);

    handle.set_lines(["before"]);
    tui.start();
    tui.wait_for_render().await;
    let redraws_before_image = tui.full_redraws();
    terminal.clear_writes();

    let image_lines = with_kitty_image_lines(3, 30);
    let mut lines = vec!["before".to_string()];
    lines.extend(image_lines);
    lines.push("after".to_string());
    handle.set_lines(lines);
    tui.request_render(false);
    tui.wait_for_render().await;

    assert!(
        tui.full_redraws() > redraws_before_image,
        "an unsafe image pre-clear forces a full redraw"
    );
    assert!(
        terminal.get_writes().contains("\x1b[2J"),
        "the fallback clears and redraws fully"
    );

    tui.stop(TuiStopOptions::default());
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn reserves_kitty_image_rows_before_drawing_during_full_redraw_fallbacks() {
    let _guard = guard();
    let terminal = VirtualTerminal::new(40, 5);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (handle, component) = test_component();
    tui.core().add_child(component);

    handle.set_lines(["l0", "l1", "l2", "l3", "l4"]);
    tui.start();
    tui.wait_for_render().await;
    let redraws_before_image = tui.full_redraws();
    terminal.clear_writes();

    let image_lines = with_kitty_image_lines(3, 30);
    let image_sequence = image_lines[0].clone();
    let mut lines: Vec<String> = ["l0", "l1", "l2", "l3", "l4"]
        .iter()
        .map(ToString::to_string)
        .collect();
    lines.extend(image_lines);
    lines.push("after".to_string());
    handle.set_lines(lines);
    tui.request_render(false);
    tui.wait_for_render().await;

    let writes = terminal.get_writes();
    assert!(
        tui.full_redraws() > redraws_before_image,
        "a scrolling image append forces a full redraw"
    );
    assert!(
        writes.contains(&format!("\r\n\r\n\x1b[2A{image_sequence}\x1b[2B")),
        "the full redraw reserves the visible image rows before drawing the placement"
    );
    assert!(
        !writes.contains(&format!("{image_sequence}\r\n\x1b[0m")),
        "the full redraw must not write reserved padding rows after the placement"
    );

    tui.stop(TuiStopOptions::default());
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn does_not_use_cursor_up_placement_for_images_taller_than_the_viewport() {
    let _guard = guard();
    let terminal = VirtualTerminal::new(40, 5);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (handle, component) = test_component();
    tui.core().add_child(component);

    handle.set_lines(["before"]);
    tui.start();
    tui.wait_for_render().await;
    terminal.clear_writes();

    let image_lines = with_kitty_image_lines(6, 60);
    let image_sequence = image_lines[0].clone();
    assert!(
        image_lines.len() > 5,
        "the test image must exceed the viewport height"
    );

    let mut lines = vec!["before".to_string()];
    lines.extend(image_lines.clone());
    lines.push("after".to_string());
    handle.set_lines(lines);
    tui.request_render(true);
    tui.wait_for_render().await;

    let writes = terminal.get_writes();
    assert!(writes.contains(&image_sequence), "the placement is drawn");
    assert!(
        !writes.contains(&format!("\x1b[{}A{image_sequence}", image_lines.len() - 1)),
        "taller-than-viewport images keep the first-row placement path"
    );

    tui.stop(TuiStopOptions::default());
}

// describe("differential rendering settles unchanged lines") — step 7 of the
// line-sharing plan: pointer identity first, content comparison as fallback.

#[tokio::test]
async fn an_unchanged_repaint_rewrites_no_line() {
    let _guard = guard();
    let terminal = VirtualTerminal::new(40, 10);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));

    // A caching component: the second frame returns the same shared lines,
    // so the pointer shortcut settles them.
    let text = component_ref(notagent_tui::Text::new("cached line", 0, 0));
    // A cacheless component: fresh allocations every frame with identical
    // content — only the content fallback can settle these.
    let (handle, component) = test_component();
    handle.set_lines(["fresh line"]);
    tui.core().add_child(text);
    tui.core().add_child(component);
    tui.start();
    tui.render_now(false);

    terminal.clear_writes();
    tui.render_now(false);
    assert!(
        !terminal.get_writes().contains("\x1b[2K"),
        "an unchanged frame must not rewrite lines: {:?}",
        terminal.get_writes()
    );
}

#[tokio::test]
async fn a_content_change_is_still_found_among_identity_less_lines() {
    let _guard = guard();
    let terminal = VirtualTerminal::new(40, 10);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (handle, component) = test_component();
    handle.set_lines(["line 0", "line 1", "line 2"]);
    tui.core().add_child(component);
    tui.start();
    tui.render_now(false);

    // Every line comes back with a new identity; only the middle one differs.
    handle.set_lines(["line 0", "changed", "line 2"]);
    terminal.clear_writes();
    tui.render_now(false);
    let writes = terminal.get_writes();
    assert!(writes.contains("changed"), "{writes:?}");
    assert_eq!(
        writes.matches("\x1b[2K").count(),
        1,
        "exactly the one changed line is rewritten: {writes:?}"
    );
}
