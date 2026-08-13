//! Port of `packages/tui/test/overlay-non-capturing.test.ts` (1203 LOC).
//!
//! This suite drives the overlay focus restore state machine
//! (`inactive | eligible | blocked` with both resume variants).

use std::cell::RefCell;
use std::rc::Rc;

use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{
    Component, ComponentRef, Focusable, OverlayHandle, OverlayOptions, OverlayUnfocusOptions,
    TuiStopOptions, component_ref,
};
use notagent_tui::tui_main_screen::TuiMainScreen;

/// `class FocusableOverlay` — focus flag and received input are observable
/// through a handle, since the component itself lives behind a `ComponentRef`.
#[derive(Clone, Default)]
struct Probe {
    focused: Rc<RefCell<bool>>,
    inputs: Rc<RefCell<Vec<String>>>,
}

impl Probe {
    fn focused(&self) -> bool {
        *self.focused.borrow()
    }

    fn inputs(&self) -> Vec<String> {
        self.inputs.borrow().clone()
    }
}

struct FocusableOverlay {
    probe: Probe,
    lines: Vec<String>,
}

impl Component for FocusableOverlay {
    fn render(&mut self, _width: usize) -> Vec<String> {
        self.lines.clone()
    }

    fn handle_input(&mut self, data: &str) {
        self.probe.inputs.borrow_mut().push(data.to_string());
    }

    fn invalidate(&mut self) {}

    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        Some(self)
    }
}

impl Focusable for FocusableOverlay {
    fn focused(&self) -> bool {
        *self.probe.focused.borrow()
    }

    fn set_focused(&mut self, focused: bool) {
        *self.probe.focused.borrow_mut() = focused;
    }
}

fn focusable(line: &str) -> (Probe, ComponentRef) {
    let probe = Probe::default();
    let component = component_ref(FocusableOverlay {
        probe: probe.clone(),
        lines: vec![line.to_string()],
    });
    (probe, component)
}

struct EmptyContent;

impl Component for EmptyContent {
    fn render(&mut self, _width: usize) -> Vec<String> {
        Vec::new()
    }
    fn invalidate(&mut self) {}
}

async fn render_and_flush(tui: &mut TuiMainScreen) {
    tui.request_render(true);
    tui.wait_for_render().await;
}

/// Standard setup: empty base content plus a focused editor.
fn setup() -> (VirtualTerminal, TuiMainScreen, Probe, ComponentRef) {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (editor, editor_component) = focusable("EDITOR");
    tui.core().add_child(component_ref(EmptyContent));
    tui.core().set_focus(Some(editor_component.clone()));
    tui.start();
    (terminal, tui, editor, editor_component)
}

fn non_capturing() -> Option<OverlayOptions> {
    Some(OverlayOptions {
        non_capturing: true,
        ..OverlayOptions::default()
    })
}

fn visible_when(flag: Rc<RefCell<bool>>) -> Option<OverlayOptions> {
    Some(OverlayOptions {
        visible: Some(Box::new(move |_, _| *flag.borrow())),
        ..OverlayOptions::default()
    })
}

// describe("focus management")

#[tokio::test]
async fn non_capturing_overlay_preserves_focus_on_creation() {
    let (_terminal, mut tui, editor, _) = setup();
    let (overlay, overlay_component) = focusable("OVERLAY");

    tui.core().show_overlay(overlay_component, non_capturing());
    render_and_flush(&mut tui).await;

    assert!(editor.focused());
    assert!(!overlay.focused());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn focus_transfers_focus_to_the_overlay() {
    let (_terminal, mut tui, editor, _) = setup();
    let (overlay, overlay_component) = focusable("OVERLAY");

    let handle = tui.core().show_overlay(overlay_component, non_capturing());
    handle.focus();
    render_and_flush(&mut tui).await;

    assert!(!editor.focused());
    assert!(overlay.focused());
    assert!(handle.is_focused());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn unfocus_restores_previous_focus() {
    let (_terminal, mut tui, editor, _) = setup();
    let (overlay, overlay_component) = focusable("OVERLAY");

    let handle = tui.core().show_overlay(overlay_component, non_capturing());
    handle.focus();
    handle.unfocus();
    render_and_flush(&mut tui).await;

    assert!(editor.focused());
    assert!(!overlay.focused());
    assert!(!handle.is_focused());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn set_hidden_false_on_non_capturing_overlay_does_not_auto_focus() {
    let (_terminal, mut tui, editor, _) = setup();
    let (overlay, overlay_component) = focusable("OVERLAY");

    let handle = tui.core().show_overlay(overlay_component, non_capturing());
    handle.set_hidden(true);
    handle.set_hidden(false);
    render_and_flush(&mut tui).await;

    assert!(editor.focused());
    assert!(!overlay.focused());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn hide_when_overlay_is_not_focused_does_not_change_focus() {
    let (_terminal, mut tui, editor, _) = setup();
    let (_overlay, overlay_component) = focusable("OVERLAY");

    let handle = tui.core().show_overlay(overlay_component, non_capturing());
    handle.hide();
    render_and_flush(&mut tui).await;

    assert!(editor.focused());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn hide_when_focused_restores_focus_correctly() {
    let (_terminal, mut tui, editor, _) = setup();
    let (overlay, overlay_component) = focusable("OVERLAY");

    let handle = tui.core().show_overlay(overlay_component, non_capturing());
    handle.focus();
    handle.hide();
    render_and_flush(&mut tui).await;

    assert!(editor.focused());
    assert!(!overlay.focused());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn capturing_overlay_removed_with_non_capturing_below_restores_focus_to_editor() {
    let (_terminal, mut tui, editor, _) = setup();
    let (non_capturing_probe, non_capturing_component) = focusable("NC");
    let (capturing, capturing_component) = focusable("CAP");

    tui.core()
        .show_overlay(non_capturing_component, non_capturing());
    let handle = tui.core().show_overlay(capturing_component, None);
    assert!(capturing.focused());
    handle.hide();
    render_and_flush(&mut tui).await;

    assert!(editor.focused());
    assert!(!non_capturing_probe.focused());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn sub_overlay_cleanup_then_hide_overlay_restores_focus_and_input_to_editor() {
    let (terminal, mut tui, editor, _) = setup();
    let (timer, timer_component) = focusable("TIMER");
    let (controller, controller_component) = focusable("CTRL");

    let timer_handle = tui.core().show_overlay(timer_component, non_capturing());
    tui.core().show_overlay(controller_component, None);
    assert!(controller.focused());
    assert!(!editor.focused());

    timer_handle.hide();
    tui.core().hide_overlay();
    render_and_flush(&mut tui).await;

    assert!(editor.focused());
    assert!(!controller.focused());
    assert!(!timer.focused());

    terminal.send_input("x");
    render_and_flush(&mut tui).await;
    assert_eq!(editor.inputs(), ["x".to_string()]);
    assert!(controller.inputs().is_empty());
    assert!(timer.inputs().is_empty());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn removed_focused_child_overlay_does_not_become_parent_overlay_fallback() {
    let (terminal, mut tui, editor, _) = setup();
    let (child, child_component) = focusable("CHILD");
    let (parent, parent_component) = focusable("PARENT");

    let child_handle = tui.core().show_overlay(child_component, non_capturing());
    child_handle.focus();
    let parent_handle = tui.core().show_overlay(parent_component, None);
    assert!(parent.focused());

    child_handle.hide();
    parent_handle.hide();
    terminal.send_input("x");
    render_and_flush(&mut tui).await;

    assert_eq!(editor.inputs(), ["x".to_string()]);
    assert!(child.inputs().is_empty());
    assert!(parent.inputs().is_empty());
    assert!(editor.focused());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn deferred_sub_overlay_pattern_restores_focus() {
    // Simulates `showExtensionCustom`: the factory creates the timer overlay
    // synchronously and the controller follows in a deferred step.
    let (terminal, mut tui, editor, _) = setup();
    let (timer, timer_component) = focusable("TIMER");
    let (controller, controller_component) = focusable("CTRL");

    let timer_handle = tui.core().show_overlay(timer_component, non_capturing());
    tokio::task::yield_now().await;
    tui.core().show_overlay(controller_component, None);
    render_and_flush(&mut tui).await;

    assert!(controller.focused());
    assert!(!editor.focused());

    // Simulate Esc: cleanup plus close.
    timer_handle.hide();
    tui.core().hide_overlay();
    render_and_flush(&mut tui).await;

    assert!(editor.focused(), "editor should regain focus");
    assert!(!controller.focused());
    assert!(!timer.focused());

    terminal.send_input("x");
    render_and_flush(&mut tui).await;
    assert_eq!(
        editor.inputs(),
        ["x".to_string()],
        "editor should receive input after close"
    );
    assert!(controller.inputs().is_empty());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn handle_input_redirection_skips_non_capturing_overlays_when_focused_overlay_becomes_invisible()
 {
    let (terminal, mut tui, _editor, _) = setup();
    let (fallback_capturing, fallback_component) = focusable("FALLBACK");
    let (non_capturing_probe, non_capturing_component) = focusable("NC");
    let (primary, primary_component) = focusable("PRIMARY");
    let is_visible = Rc::new(RefCell::new(true));

    tui.core().show_overlay(fallback_component, None);
    tui.core()
        .show_overlay(non_capturing_component, non_capturing());
    tui.core()
        .show_overlay(primary_component, visible_when(Rc::clone(&is_visible)));
    assert!(primary.focused());

    *is_visible.borrow_mut() = false;
    terminal.send_input("x");
    render_and_flush(&mut tui).await;

    assert!(primary.inputs().is_empty());
    assert!(non_capturing_probe.inputs().is_empty());
    assert_eq!(fallback_capturing.inputs(), ["x".to_string()]);
    assert!(fallback_capturing.focused());
    tui.stop(TuiStopOptions::default());
}

/// Component whose `handle_input` calls back into the TUI, like the inline
/// `handleInput` overrides of the TS suite.
struct ReactiveOverlay {
    probe: Probe,
    lines: Vec<String>,
    #[allow(clippy::type_complexity)]
    on_input: Box<dyn FnMut(&str)>,
}

impl Component for ReactiveOverlay {
    fn render(&mut self, _width: usize) -> Vec<String> {
        self.lines.clone()
    }

    fn handle_input(&mut self, data: &str) {
        self.probe.inputs.borrow_mut().push(data.to_string());
        (self.on_input)(data);
    }

    fn invalidate(&mut self) {}

    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        Some(self)
    }
}

impl Focusable for ReactiveOverlay {
    fn focused(&self) -> bool {
        *self.probe.focused.borrow()
    }

    fn set_focused(&mut self, focused: bool) {
        *self.probe.focused.borrow_mut() = focused;
    }
}

fn reactive(line: &str, on_input: Box<dyn FnMut(&str)>) -> (Probe, ComponentRef) {
    let probe = Probe::default();
    let component = component_ref(ReactiveOverlay {
        probe: probe.clone(),
        lines: vec![line.to_string()],
        on_input,
    });
    (probe, component)
}

#[tokio::test]
async fn active_base_focus_replacement_receives_close_input_before_overlay_restore() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let core = tui.core().clone();
    let (editor, editor_component) = focusable("EDITOR");

    let (replacement, replacement_component) = {
        let core = core.clone();
        let editor_component = editor_component.clone();
        reactive(
            "REPLACEMENT",
            Box::new(move |data| {
                if data == "\r" {
                    core.set_focus(Some(editor_component.clone()));
                }
            }),
        )
    };
    let (overlay, overlay_component) = {
        let core = core.clone();
        let replacement_component = replacement_component.clone();
        reactive(
            "OVERLAY",
            Box::new(move |data| {
                if data == "b" {
                    core.set_focus(Some(replacement_component.clone()));
                }
            }),
        )
    };

    tui.core().add_child(component_ref(EmptyContent));
    tui.core().set_focus(Some(editor_component));
    tui.start();

    tui.core().show_overlay(overlay_component, None);
    assert!(overlay.focused());

    terminal.send_input("b");
    render_and_flush(&mut tui).await;
    assert!(replacement.focused());

    terminal.send_input("\r");
    render_and_flush(&mut tui).await;
    assert_eq!(replacement.inputs(), ["\r".to_string()]);
    assert_eq!(overlay.inputs(), ["b".to_string()]);
    assert!(overlay.focused());

    terminal.send_input("x");
    render_and_flush(&mut tui).await;
    assert_eq!(overlay.inputs(), ["b".to_string(), "x".to_string()]);
    let _ = editor;
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn active_replacement_still_receives_input_when_it_is_another_overlay_pre_focus() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let core = tui.core().clone();
    let (_editor, editor_component) = focusable("EDITOR");
    let (_passive, passive_component) = focusable("PASSIVE");

    let (replacement, replacement_component) = {
        let core = core.clone();
        let editor_component = editor_component.clone();
        reactive(
            "REPLACEMENT",
            Box::new(move |data| {
                if data == "\r" {
                    core.set_focus(Some(editor_component.clone()));
                }
            }),
        )
    };
    let (overlay, overlay_component) = {
        let core = core.clone();
        let replacement_component = replacement_component.clone();
        reactive(
            "OVERLAY",
            Box::new(move |data| {
                if data == "b" {
                    core.set_focus(Some(replacement_component.clone()));
                }
            }),
        )
    };

    tui.core().add_child(component_ref(EmptyContent));
    tui.core().set_focus(Some(editor_component.clone()));
    tui.start();

    tui.core().set_focus(Some(replacement_component));
    tui.core().show_overlay(passive_component, non_capturing());
    tui.core().set_focus(Some(editor_component));
    tui.core().show_overlay(overlay_component, None);

    terminal.send_input("b");
    render_and_flush(&mut tui).await;
    assert!(replacement.focused());

    terminal.send_input("1");
    terminal.send_input("\r");
    render_and_flush(&mut tui).await;
    assert_eq!(replacement.inputs(), ["1".to_string(), "\r".to_string()]);
    assert_eq!(overlay.inputs(), ["b".to_string()]);
    assert!(overlay.focused());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn unfocus_target_releases_a_blocked_overlay_while_replacement_remains_focused() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let core = tui.core().clone();
    let (fallback, fallback_component) = focusable("FALLBACK");
    let (target, target_component) = focusable("TARGET");

    let (replacement, replacement_component) = {
        let core = core.clone();
        let fallback_component = fallback_component.clone();
        reactive(
            "REPLACEMENT",
            Box::new(move |data| {
                if data == "\r" {
                    core.set_focus(Some(fallback_component.clone()));
                }
            }),
        )
    };

    tui.core().add_child(component_ref(EmptyContent));
    tui.start();

    // The overlay handle is needed inside the overlay's own input handler, so
    // the handler is installed through a shared slot (TS assigns handleInput
    // after showOverlay for the same reason).
    let pending: Rc<RefCell<Option<(OverlayHandle, ComponentRef)>>> = Rc::new(RefCell::new(None));
    let (overlay, overlay_component) = {
        let core = core.clone();
        let pending = Rc::clone(&pending);
        let replacement_component = replacement_component.clone();
        let target_component = target_component.clone();
        reactive(
            "OVERLAY",
            Box::new(move |data| {
                if data == "b" {
                    core.set_focus(Some(replacement_component.clone()));
                    if let Some((handle, _)) = pending.borrow().as_ref() {
                        handle.unfocus_to(OverlayUnfocusOptions {
                            target: Some(target_component.clone()),
                        });
                    }
                }
            }),
        )
    };
    let handle = tui.core().show_overlay(overlay_component.clone(), None);
    *pending.borrow_mut() = Some((handle, overlay_component));

    terminal.send_input("b");
    render_and_flush(&mut tui).await;
    assert!(replacement.focused());

    terminal.send_input("\r");
    terminal.send_input("x");
    render_and_flush(&mut tui).await;

    assert_eq!(overlay.inputs(), ["b".to_string()]);
    assert_eq!(replacement.inputs(), ["\r".to_string()]);
    assert!(fallback.inputs().is_empty());
    assert_eq!(target.inputs(), ["x".to_string()]);
    tui.stop(TuiStopOptions::default());
}
