//! Port of `packages/tui/test/overlay-non-capturing.test.ts` (1203 LOC).
//!
//! This suite drives the overlay focus restore state machine
//! (`inactive | eligible | blocked` with both resume variants).

use std::cell::RefCell;
use std::rc::Rc;

use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{
    Component, ComponentRef, Container, Focusable, Line, OverlayHandle, OverlayOptions,
    OverlayUnfocusOptions, TuiStopOptions, component_ref, shared_lines,
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
    fn render(&mut self, _width: usize) -> Vec<Line> {
        shared_lines(self.lines.clone())
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
    fn render(&mut self, _width: usize) -> Vec<Line> {
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
    fn render(&mut self, _width: usize) -> Vec<Line> {
        shared_lines(self.lines.clone())
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

#[tokio::test]
async fn handle_input_restores_focus_to_a_visible_focused_overlay_after_base_focus_steal() {
    let (terminal, mut tui, editor, editor_component) = setup();
    let (_replacement, replacement_component) = focusable("REPLACEMENT");
    let (overlay, overlay_component) = focusable("OVERLAY");

    tui.core().show_overlay(overlay_component, None);
    assert!(overlay.focused());
    tui.core().set_focus(Some(replacement_component));
    tui.core().set_focus(Some(editor_component));
    terminal.send_input("x");
    render_and_flush(&mut tui).await;

    assert_eq!(overlay.inputs(), ["x".to_string()]);
    assert!(editor.inputs().is_empty());
    assert!(overlay.focused());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn handle_input_restores_focus_to_explicitly_focused_raw_sub_overlay_after_base_focus_steal()
{
    let (terminal, mut tui, editor, editor_component) = setup();
    let (controller, controller_component) = focusable("CONTROLLER");
    let (sub_overlay, sub_component) = focusable("SUB");

    tui.core().show_overlay(controller_component, None);
    let sub_handle = tui.core().show_overlay(sub_component, non_capturing());
    sub_handle.focus();
    tui.core().set_focus(Some(editor_component));
    terminal.send_input("x");
    render_and_flush(&mut tui).await;

    assert_eq!(sub_overlay.inputs(), ["x".to_string()]);
    assert!(controller.inputs().is_empty());
    assert!(editor.inputs().is_empty());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn passive_non_capturing_overlay_does_not_regain_input_after_base_focus() {
    let (terminal, mut tui, editor, _) = setup();
    let (passive, passive_component) = focusable("PASSIVE");

    tui.core().show_overlay(passive_component, non_capturing());
    terminal.send_input("x");
    render_and_flush(&mut tui).await;

    assert_eq!(editor.inputs(), ["x".to_string()]);
    assert!(passive.inputs().is_empty());
    assert!(editor.focused());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn explicitly_focused_non_capturing_overlay_regains_input_after_base_focus_steal() {
    let (terminal, mut tui, editor, editor_component) = setup();
    let (overlay, overlay_component) = focusable("NC");

    let handle = tui.core().show_overlay(overlay_component, non_capturing());
    handle.focus();
    tui.core().set_focus(Some(editor_component));
    terminal.send_input("x");
    render_and_flush(&mut tui).await;

    assert_eq!(overlay.inputs(), ["x".to_string()]);
    assert!(editor.inputs().is_empty());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn unfocus_prevents_visible_overlay_from_regaining_input() {
    let (terminal, mut tui, editor, _) = setup();
    let (overlay, overlay_component) = focusable("OVERLAY");

    let handle = tui.core().show_overlay(overlay_component, None);
    handle.unfocus();
    terminal.send_input("x");
    render_and_flush(&mut tui).await;

    assert_eq!(editor.inputs(), ["x".to_string()]);
    assert!(overlay.inputs().is_empty());
    assert!(editor.focused());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn set_focus_null_explicitly_clears_visible_overlay_restore() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (overlay, overlay_component) = focusable("OVERLAY");
    tui.core().add_child(component_ref(EmptyContent));
    tui.start();

    tui.core().show_overlay(overlay_component, None);
    tui.core().set_focus(None);
    terminal.send_input("x");
    render_and_flush(&mut tui).await;

    assert!(overlay.inputs().is_empty());
    assert!(!overlay.focused());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn blocked_replacement_set_focus_null_resumes_the_visible_overlay() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let core = tui.core().clone();

    let (replacement, replacement_component) = {
        let core = core.clone();
        reactive(
            "REPLACEMENT",
            Box::new(move |data| {
                if data == "\r" {
                    core.set_focus(None);
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
    tui.start();
    tui.core().show_overlay(overlay_component, None);

    terminal.send_input("b");
    render_and_flush(&mut tui).await;
    terminal.send_input("\r");
    terminal.send_input("x");
    render_and_flush(&mut tui).await;

    assert_eq!(replacement.inputs(), ["\r".to_string()]);
    assert_eq!(overlay.inputs(), ["b".to_string(), "x".to_string()]);
    assert!(overlay.focused());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn temporarily_invisible_focused_overlay_falls_back_without_losing_restore_eligibility() {
    let (terminal, mut tui, editor, editor_component) = setup();
    let (overlay, overlay_component) = focusable("OVERLAY");
    let visible = Rc::new(RefCell::new(true));

    tui.core()
        .show_overlay(overlay_component, visible_when(Rc::clone(&visible)));
    tui.core().set_focus(Some(editor_component));
    *visible.borrow_mut() = false;
    terminal.send_input("x");
    render_and_flush(&mut tui).await;

    assert_eq!(editor.inputs(), ["x".to_string()]);
    assert!(overlay.inputs().is_empty());

    *visible.borrow_mut() = true;
    terminal.send_input("y");
    render_and_flush(&mut tui).await;

    assert_eq!(editor.inputs(), ["x".to_string()]);
    assert_eq!(overlay.inputs(), ["y".to_string()]);
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn temporarily_invisible_focused_overlay_with_null_pre_focus_restores_when_visible_again() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (overlay, overlay_component) = focusable("OVERLAY");
    let visible = Rc::new(RefCell::new(true));
    tui.core().add_child(component_ref(EmptyContent));
    tui.start();

    tui.core()
        .show_overlay(overlay_component, visible_when(Rc::clone(&visible)));
    *visible.borrow_mut() = false;
    terminal.send_input("x");
    render_and_flush(&mut tui).await;
    assert!(overlay.inputs().is_empty());

    *visible.borrow_mut() = true;
    terminal.send_input("y");
    render_and_flush(&mut tui).await;
    assert_eq!(overlay.inputs(), ["y".to_string()]);
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn cyclic_overlay_pre_focus_ancestry_does_not_hang_focus_changes() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (editor, editor_component) = focusable("EDITOR");
    let (overlay, overlay_component) = focusable("OVERLAY");
    tui.core().add_child(component_ref(EmptyContent));
    tui.core().set_focus(Some(overlay_component.clone()));
    tui.start();

    let handle = tui.core().show_overlay(overlay_component, non_capturing());
    handle.focus();
    tui.core().set_focus(Some(editor_component));
    terminal.send_input("x");
    render_and_flush(&mut tui).await;

    assert_eq!(editor.inputs(), ["x".to_string()]);
    assert!(overlay.inputs().is_empty());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn handle_input_restores_the_focus_order_top_overlay_after_base_focus_steal() {
    let (terminal, mut tui, editor, editor_component) = setup();
    let (lower, lower_component) = focusable("LOWER");
    let (upper, upper_component) = focusable("UPPER");

    let lower_handle = tui.core().show_overlay(lower_component, None);
    tui.core().show_overlay(upper_component, None);
    lower_handle.focus();
    tui.core().set_focus(Some(editor_component));
    terminal.send_input("x");
    render_and_flush(&mut tui).await;

    assert_eq!(lower.inputs(), ["x".to_string()]);
    assert!(upper.inputs().is_empty());
    assert!(editor.inputs().is_empty());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn hide_overlay_does_not_reassign_focus_when_topmost_overlay_is_non_capturing() {
    let (_terminal, mut tui, _editor, _) = setup();
    let (capturing, capturing_component) = focusable("CAP");
    let (_nc, nc_component) = focusable("NC");

    tui.core().show_overlay(capturing_component, None);
    tui.core().show_overlay(nc_component, non_capturing());
    assert!(capturing.focused());

    tui.core().hide_overlay();
    render_and_flush(&mut tui).await;
    assert!(capturing.focused());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn multiple_capturing_and_non_capturing_overlays_restore_focus_through_removals() {
    let (_terminal, mut tui, editor, _) = setup();
    let (c1, c1_component) = focusable("C1");
    let (_n1, n1_component) = focusable("N1");
    let (c2, c2_component) = focusable("C2");
    let (_n2, n2_component) = focusable("N2");

    let c1_handle = tui.core().show_overlay(c1_component, None);
    tui.core().show_overlay(n1_component, non_capturing());
    let c2_handle = tui.core().show_overlay(c2_component, None);
    tui.core().show_overlay(n2_component, non_capturing());
    assert!(c2.focused());

    c2_handle.hide();
    render_and_flush(&mut tui).await;
    assert!(c1.focused());

    c1_handle.hide();
    render_and_flush(&mut tui).await;
    assert!(editor.focused());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn capturing_overlay_unfocus_on_topmost_capturing_overlay_falls_back_to_pre_focus() {
    let (_terminal, mut tui, editor, _) = setup();
    let (capturing, capturing_component) = focusable("CAP");

    let handle = tui.core().show_overlay(capturing_component, None);
    assert!(capturing.focused());
    handle.unfocus();
    render_and_flush(&mut tui).await;

    assert!(editor.focused());
    assert!(!capturing.focused());
    tui.stop(TuiStopOptions::default());
}

// describe("no-op guards")

#[tokio::test]
async fn focus_on_hidden_overlay_is_a_no_op() {
    let (_terminal, mut tui, editor, _) = setup();
    let (_overlay, overlay_component) = focusable("OVERLAY");

    let handle = tui.core().show_overlay(overlay_component, non_capturing());
    handle.set_hidden(true);
    handle.focus();
    render_and_flush(&mut tui).await;

    assert!(editor.focused());
    assert!(!handle.is_focused());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn focus_after_hide_is_a_no_op() {
    let (_terminal, mut tui, editor, _) = setup();
    let (_overlay, overlay_component) = focusable("OVERLAY");

    let handle = tui.core().show_overlay(overlay_component, non_capturing());
    handle.hide();
    handle.focus();
    render_and_flush(&mut tui).await;

    assert!(editor.focused());
    assert!(!handle.is_focused());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn unfocus_when_overlay_does_not_have_focus_is_a_no_op() {
    let (_terminal, mut tui, editor, _) = setup();
    let (overlay, overlay_component) = focusable("OVERLAY");

    let handle = tui.core().show_overlay(overlay_component, non_capturing());
    handle.unfocus();
    render_and_flush(&mut tui).await;

    assert!(editor.focused());
    assert!(!overlay.focused());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn unfocus_with_null_pre_focus_clears_focus_and_does_not_route_input_back() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (overlay, overlay_component) = focusable("OVERLAY");
    tui.core().add_child(component_ref(EmptyContent));
    tui.start();

    let handle = tui.core().show_overlay(overlay_component, None);
    assert!(overlay.focused());
    handle.unfocus();
    assert!(!overlay.focused());
    terminal.send_input("x");
    render_and_flush(&mut tui).await;

    assert!(overlay.inputs().is_empty());
    assert!(!handle.is_focused());
    tui.stop(TuiStopOptions::default());
}

// describe("focus cycle prevention")

#[tokio::test]
async fn toggle_focus_between_non_capturing_overlays_then_unfocus_returns_to_editor() {
    let (_terminal, mut tui, editor, _) = setup();
    let (a, a_component) = focusable("A");
    let (b, b_component) = focusable("B");

    let a_handle = tui.core().show_overlay(a_component, non_capturing());
    let b_handle = tui.core().show_overlay(b_component, non_capturing());
    a_handle.focus();
    b_handle.focus();
    a_handle.focus();
    a_handle.unfocus();
    render_and_flush(&mut tui).await;

    assert!(editor.focused());
    assert!(!a.focused());
    assert!(!b.focused());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn explicit_unfocus_target_supports_cycling_between_three_overlays_and_editor() {
    let (terminal, mut tui, editor, editor_component) = setup();
    let (a, a_component) = focusable("A");
    let (b, b_component) = focusable("B");
    let (c, c_component) = focusable("C");

    let a_handle = tui.core().show_overlay(a_component, None);
    let b_handle = tui.core().show_overlay(b_component, None);
    let c_handle = tui.core().show_overlay(c_component, None);

    a_handle.focus();
    terminal.send_input("a");
    render_and_flush(&mut tui).await;
    b_handle.focus();
    terminal.send_input("b");
    render_and_flush(&mut tui).await;
    c_handle.focus();
    terminal.send_input("c");
    render_and_flush(&mut tui).await;
    c_handle.unfocus_to(OverlayUnfocusOptions {
        target: Some(editor_component.clone()),
    });
    terminal.send_input("e");
    render_and_flush(&mut tui).await;
    a_handle.focus();
    terminal.send_input("A");
    render_and_flush(&mut tui).await;
    a_handle.unfocus_to(OverlayUnfocusOptions {
        target: Some(editor_component),
    });
    terminal.send_input("E");
    render_and_flush(&mut tui).await;

    assert_eq!(a.inputs(), ["a".to_string(), "A".to_string()]);
    assert_eq!(b.inputs(), ["b".to_string()]);
    assert_eq!(c.inputs(), ["c".to_string()]);
    assert_eq!(editor.inputs(), ["e".to_string(), "E".to_string()]);
    assert!(editor.focused());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn explicit_null_unfocus_target_clears_focus_without_restoring_overlays() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (overlay, overlay_component) = focusable("OVERLAY");
    tui.core().add_child(component_ref(EmptyContent));
    tui.start();

    let handle = tui.core().show_overlay(overlay_component, None);
    handle.unfocus_to(OverlayUnfocusOptions { target: None });
    terminal.send_input("x");
    render_and_flush(&mut tui).await;

    assert!(overlay.inputs().is_empty());
    assert!(!handle.is_focused());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn hiding_focused_overlay_falls_back_to_next_visual_frontmost_overlay() {
    let (terminal, mut tui, _editor, _) = setup();
    let (a, a_component) = focusable("A");
    let (b, b_component) = focusable("B");
    let (c, c_component) = focusable("C");

    let a_handle = tui.core().show_overlay(a_component, None);
    let b_handle = tui.core().show_overlay(b_component, None);
    tui.core().show_overlay(c_component, None);
    a_handle.focus();
    b_handle.focus();
    b_handle.set_hidden(true);
    terminal.send_input("x");
    render_and_flush(&mut tui).await;

    assert_eq!(a.inputs(), ["x".to_string()]);
    assert!(c.inputs().is_empty());
    assert!(a.focused());
    let _ = b;
    tui.stop(TuiStopOptions::default());
}

// describe("rendering order")

/// `class StaticOverlay` of the TS suite.
struct StaticOverlay(Vec<String>);

impl Component for StaticOverlay {
    fn render(&mut self, _width: usize) -> Vec<Line> {
        shared_lines(self.0.clone())
    }
    fn invalidate(&mut self) {}
}

fn corner_overlay(non_capturing: bool) -> Option<OverlayOptions> {
    Some(OverlayOptions {
        row: Some(notagent_tui::tui::SizeValue::Cells(0)),
        col: Some(notagent_tui::tui::SizeValue::Cells(0)),
        width: Some(notagent_tui::tui::SizeValue::Cells(1)),
        non_capturing,
        ..OverlayOptions::default()
    })
}

fn first_char(terminal: &VirtualTerminal) -> Option<char> {
    terminal.get_viewport().first()?.chars().next()
}

#[tokio::test]
async fn focus_on_already_focused_overlay_bumps_visual_order() {
    let terminal = VirtualTerminal::new(20, 6);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (_editor, editor_component) = focusable("EDITOR");
    tui.core().add_child(component_ref(EmptyContent));
    tui.core().set_focus(Some(editor_component));
    tui.start();

    let a_handle = tui.core().show_overlay(
        component_ref(StaticOverlay(vec!["A".to_string()])),
        corner_overlay(true),
    );
    tui.core().show_overlay(
        component_ref(StaticOverlay(vec!["B".to_string()])),
        corner_overlay(true),
    );
    a_handle.focus();
    tui.core().show_overlay(
        component_ref(StaticOverlay(vec!["C".to_string()])),
        corner_overlay(true),
    );
    render_and_flush(&mut tui).await;
    assert_eq!(first_char(&terminal), Some('C'));

    a_handle.focus();
    render_and_flush(&mut tui).await;
    assert_eq!(first_char(&terminal), Some('A'));
    assert!(a_handle.is_focused());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn default_rendering_order_for_overlapping_overlays_follows_creation_order() {
    let terminal = VirtualTerminal::new(20, 6);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    tui.core().add_child(component_ref(EmptyContent));
    tui.start();

    tui.core().show_overlay(
        component_ref(StaticOverlay(vec!["A".to_string()])),
        corner_overlay(true),
    );
    tui.core().show_overlay(
        component_ref(StaticOverlay(vec!["B".to_string()])),
        corner_overlay(true),
    );
    render_and_flush(&mut tui).await;
    assert_eq!(first_char(&terminal), Some('B'));
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn focus_on_lower_overlay_renders_it_on_top() {
    let terminal = VirtualTerminal::new(20, 6);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    tui.core().add_child(component_ref(EmptyContent));
    tui.start();

    let lower = tui.core().show_overlay(
        component_ref(StaticOverlay(vec!["A".to_string()])),
        corner_overlay(true),
    );
    tui.core().show_overlay(
        component_ref(StaticOverlay(vec!["B".to_string()])),
        corner_overlay(true),
    );
    render_and_flush(&mut tui).await;
    assert_eq!(first_char(&terminal), Some('B'));

    lower.focus();
    render_and_flush(&mut tui).await;
    assert_eq!(first_char(&terminal), Some('A'));
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn focusing_middle_overlay_places_it_on_top_while_preserving_relative_order() {
    let terminal = VirtualTerminal::new(20, 6);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    tui.core().add_child(component_ref(EmptyContent));
    tui.start();

    tui.core().show_overlay(
        component_ref(StaticOverlay(vec!["A".to_string()])),
        corner_overlay(true),
    );
    let middle = tui.core().show_overlay(
        component_ref(StaticOverlay(vec!["B".to_string()])),
        corner_overlay(true),
    );
    let top = tui.core().show_overlay(
        component_ref(StaticOverlay(vec!["C".to_string()])),
        corner_overlay(true),
    );
    render_and_flush(&mut tui).await;
    assert_eq!(first_char(&terminal), Some('C'));

    middle.focus();
    render_and_flush(&mut tui).await;
    assert_eq!(first_char(&terminal), Some('B'));

    middle.hide();
    render_and_flush(&mut tui).await;
    assert_eq!(first_char(&terminal), Some('C'));

    top.hide();
    render_and_flush(&mut tui).await;
    assert_eq!(first_char(&terminal), Some('A'));
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn capturing_overlay_hidden_and_shown_again_renders_on_top_after_unhide() {
    let terminal = VirtualTerminal::new(20, 6);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    tui.core().add_child(component_ref(EmptyContent));
    tui.start();

    tui.core().show_overlay(
        component_ref(StaticOverlay(vec!["A".to_string()])),
        corner_overlay(true),
    );
    let capturing = tui.core().show_overlay(
        component_ref(StaticOverlay(vec!["B".to_string()])),
        corner_overlay(false),
    );
    render_and_flush(&mut tui).await;
    assert_eq!(first_char(&terminal), Some('B'));

    capturing.set_hidden(true);
    tui.core().show_overlay(
        component_ref(StaticOverlay(vec!["C".to_string()])),
        corner_overlay(true),
    );
    render_and_flush(&mut tui).await;
    assert_eq!(first_char(&terminal), Some('C'));

    capturing.set_hidden(false);
    render_and_flush(&mut tui).await;
    assert_eq!(first_char(&terminal), Some('B'));
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn unfocus_does_not_change_visual_order_until_another_overlay_is_focused() {
    let terminal = VirtualTerminal::new(20, 6);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let (_editor, editor_component) = focusable("EDITOR");
    tui.core().add_child(component_ref(EmptyContent));
    tui.core().set_focus(Some(editor_component));
    tui.start();

    let a = tui.core().show_overlay(
        component_ref(StaticOverlay(vec!["A".to_string()])),
        corner_overlay(true),
    );
    let b = tui.core().show_overlay(
        component_ref(StaticOverlay(vec!["B".to_string()])),
        corner_overlay(true),
    );
    render_and_flush(&mut tui).await;
    assert_eq!(first_char(&terminal), Some('B'));

    a.focus();
    render_and_flush(&mut tui).await;
    assert_eq!(first_char(&terminal), Some('A'));

    a.unfocus();
    render_and_flush(&mut tui).await;
    assert_eq!(first_char(&terminal), Some('A'));

    b.focus();
    render_and_flush(&mut tui).await;
    assert_eq!(first_char(&terminal), Some('B'));
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn blocked_replacement_can_move_focus_internally_before_overlay_restore() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let core = tui.core().clone();

    let base: Rc<RefCell<Container>> = Rc::new(RefCell::new(Container::new()));
    let (_editor, editor_component) = focusable("EDITOR");

    let (second, second_component) = {
        let core = core.clone();
        let base = Rc::clone(&base);
        let editor_component = editor_component.clone();
        reactive(
            "SECOND",
            Box::new(move |data| {
                if data == "\r" {
                    base.borrow_mut().clear();
                    base.borrow_mut().add_child(editor_component.clone());
                    core.set_focus(Some(editor_component.clone()));
                }
            }),
        )
    };
    let (first, first_component) = {
        let core = core.clone();
        let second_component = second_component.clone();
        reactive(
            "FIRST",
            Box::new(move |data| {
                if data == "n" {
                    core.set_focus(Some(second_component.clone()));
                }
            }),
        )
    };
    let (overlay, overlay_component) = {
        let core = core.clone();
        let first_component = first_component.clone();
        reactive(
            "OVERLAY",
            Box::new(move |data| {
                if data == "b" {
                    core.set_focus(Some(first_component.clone()));
                }
            }),
        )
    };

    base.borrow_mut().add_child(editor_component.clone());
    base.borrow_mut().add_child(first_component);
    base.borrow_mut().add_child(second_component);
    tui.core().add_child(base.clone() as ComponentRef);
    tui.core().set_focus(Some(editor_component));
    tui.start();

    tui.core().show_overlay(overlay_component, None);
    terminal.send_input("b");
    render_and_flush(&mut tui).await;
    terminal.send_input("n");
    render_and_flush(&mut tui).await;
    terminal.send_input("2");
    terminal.send_input("\r");
    render_and_flush(&mut tui).await;

    assert_eq!(overlay.inputs(), ["b".to_string()]);
    assert_eq!(first.inputs(), ["n".to_string()]);
    assert_eq!(second.inputs(), ["2".to_string(), "\r".to_string()]);
    assert!(overlay.focused());
    tui.stop(TuiStopOptions::default());
}

#[tokio::test]
async fn removed_replacement_restores_overlay_even_when_pre_focus_differs_from_next_focus() {
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiMainScreen::new(Box::new(terminal.clone()));
    let core = tui.core().clone();

    let base: Rc<RefCell<Container>> = Rc::new(RefCell::new(Container::new()));
    let (editor, editor_component) = focusable("EDITOR");
    let (_palette, palette_component) = focusable("PALETTE");

    let (replacement, replacement_component) = {
        let core = core.clone();
        let base = Rc::clone(&base);
        let editor_component = editor_component.clone();
        reactive(
            "REPLACEMENT",
            Box::new(move |data| {
                if data == "\r" {
                    base.borrow_mut().clear();
                    base.borrow_mut().add_child(editor_component.clone());
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

    base.borrow_mut().add_child(editor_component);
    base.borrow_mut().add_child(palette_component.clone());
    base.borrow_mut().add_child(replacement_component);
    tui.core().add_child(base.clone() as ComponentRef);
    tui.core().set_focus(Some(palette_component));
    tui.start();

    tui.core().show_overlay(overlay_component, None);
    terminal.send_input("b");
    render_and_flush(&mut tui).await;
    terminal.send_input("\r");
    terminal.send_input("x");
    render_and_flush(&mut tui).await;

    assert_eq!(overlay.inputs(), ["b".to_string(), "x".to_string()]);
    assert_eq!(replacement.inputs(), ["\r".to_string()]);
    assert!(editor.inputs().is_empty());
    assert!(overlay.focused());
    tui.stop(TuiStopOptions::default());
}
