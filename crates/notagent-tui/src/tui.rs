//! Core abstractions of the component model.
//!
//! 1:1 port of `packages/tui/src/tui.ts` (1257 LOC): `Component`, `Container`,
//! `TuiBase` (here `TuiCore`), the overlay stack with its focus restore state
//! machine, render scheduling, input dispatch and `compositeTuiLine`.
//!
//! Shape changes against the TS original (deviation class 1, behaviour
//! identical):
//! - Components are shared through [`ComponentRef`], mirroring the shared
//!   object references of TS; identity comparisons use `Rc::ptr_eq`.
//! - `TuiBase` is an abstract class; Rust splits it into [`TuiCore`] (shared
//!   state, cloneable handle) plus the concrete renderers that own one and
//!   implement `do_render`.
//! - Timers do not call back into the TUI. [`TuiCore::render_deadline`] reports
//!   when the next frame is due and the renderer's loop performs it, so all
//!   component access stays on one thread like the Node event loop.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::keys::{is_key_release, matches_key};
use crate::terminal::Terminal;
use crate::terminal_colors::{
    RgbColor, TerminalColorScheme, is_osc11_background_color_response,
    parse_osc11_background_color, parse_terminal_color_scheme_report,
};
use crate::terminal_image::{CellDimensions, get_capabilities, is_image_line, set_cell_dimensions};
use crate::utils::{
    extract_segments, normalize_terminal_output, slice_by_column, slice_with_width, visible_width,
};

/// Shared reference to a component.
///
/// In TS both the container and the caller hold the same component instance and
/// mutate it (`editor.setText(...)` after `tui.addChild(editor)`).
/// `Rc<RefCell<…>>` mirrors that reference semantics; the TUI core is
/// single-threaded like the TS original.
pub type ComponentRef = Rc<RefCell<dyn Component>>;

/// Wrap a component into a [`ComponentRef`].
pub fn component_ref<C: Component + 'static>(component: C) -> ComponentRef {
    Rc::new(RefCell::new(component))
}

/// Renderer a terminal loop can drive: the main screen or the alternate screen.
///
/// Deviation class 1: TS needs no such abstraction because `scheduleRender()`
/// hands the frame to `setTimeout` and the Node event loop calls back into the
/// renderer (`packages/tui/src/tui.ts:243-258`). Rust has no ambient loop, so
/// the loop is written once against this trait.
pub trait RenderLoop {
    /// Shared state of this renderer.
    fn core(&self) -> &TuiCore;
    /// Render the pending frame, if one is pending.
    fn render_pending_frame(&mut self);
}

/// Drive rendering and terminal input until `until` resolves, then return its
/// output.
///
/// This is the Rust stand-in for what Node does while a TS dialog awaits its
/// promise: `startStartupTui` starts the TUI and returns, and the event loop
/// keeps delivering stdin and rendering until the dialog resolves
/// (`packages/coding-agent/src/cli/startup-ui.ts:86-90`,
/// `packages/coding-agent/src/cli/session-picker.ts:20-55`). After stdin hits
/// EOF the loop keeps rendering, exactly as Node does once the `data` handler
/// stops firing.
pub async fn run_until<R, F>(
    ui: &mut R,
    pump: &mut dyn crate::terminal::TerminalPump,
    until: F,
) -> F::Output
where
    R: RenderLoop + ?Sized,
    F: std::future::Future,
{
    tokio::pin!(until);
    let mut stdin_open = true;
    loop {
        let core = ui.core().clone();
        tokio::select! {
            biased;
            output = &mut until => return output,
            () = core.wait_until_render_due() => ui.render_pending_frame(),
            result = async {
                if stdin_open {
                    pump.pump().await
                } else {
                    std::future::pending().await
                }
            } => {
                if result == crate::terminal::PumpResult::Eof {
                    stdin_open = false;
                }
            }
        }
    }
}

/// A rendered line, shared instead of copied.
///
/// Deliberate deviation from the TS original, where strings are immutable and
/// shared by the runtime, so returning a cached line costs nothing. Shared
/// rather than borrowed because a container flattens its children's lines into
/// one list (`Container::render`) and cannot hold a borrow into every child it
/// visited. Atomic (`Arc`, not `Rc`) to stay comparable with the reference
/// (`../notagent-main-rust/crates/notagent_tui/src/component.rs:23`); a check
/// of `components/loader.rs` and `components/cancellable_loader.rs` found no
/// rendered line crossing a thread here today — they animate via the render
/// loop, not on their own threads — and the atomic refcount costs nanoseconds.
/// The second win sits in the screen diff: an unchanged cached line can settle
/// by pointer identity before any byte comparison runs.
pub type Line = std::sync::Arc<str>;

/// Convert freshly built owned lines into shared ones at the return edge.
///
/// For components that assemble their lines as owned strings and do not cache;
/// components with a cache store `Line`s directly so a hit is a refcount bump.
pub fn shared_lines(lines: Vec<String>) -> Vec<Line> {
    lines.into_iter().map(Line::from).collect()
}

/// Normalize `line` and terminate it with the segment reset — exactly the
/// transformation `apply_line_resets` applies to a non-image line.
///
/// Factored out so a caching component can store finished lines
/// (`components/markdown.rs`): the paint pass then skips them, which keeps
/// their pointer identity across frames for the screen diff.
pub(crate) fn finish_line(line: &str) -> String {
    match normalize_terminal_output(line) {
        std::borrow::Cow::Borrowed(text) => {
            let mut owned = String::with_capacity(text.len() + SEGMENT_RESET.len());
            owned.push_str(text);
            owned.push_str(SEGMENT_RESET);
            owned
        }
        std::borrow::Cow::Owned(mut owned) => {
            owned.push_str(SEGMENT_RESET);
            owned
        }
    }
}

/// Component interface — every component implements it.
///
/// Corresponds to `interface Component` (`packages/tui/src/tui.ts:23-46`).
pub trait Component {
    /// Render the component for the given viewport width.
    ///
    /// Returns one shared string per line (with embedded ANSI sequences).
    fn render(&mut self, width: usize) -> Vec<Line>;

    /// Optional handler for keyboard input while the component has focus.
    ///
    /// `handleInput` is optional in TS; the default implementation here behaves
    /// like the missing method.
    fn handle_input(&mut self, data: &str) {
        let _ = data;
    }

    /// If `true` the component receives key release events (Kitty protocol).
    /// Default is `false` — release events are filtered out.
    fn wants_key_release(&self) -> bool {
        false
    }

    /// Drop cached render state.
    fn invalidate(&mut self);

    /// Replaces the TS type guard `isFocusable()` (`"focused" in component`).
    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        None
    }

    /// Replaces the TS check `root instanceof Container` used when walking the
    /// mounted component tree.
    fn as_container(&self) -> Option<&Container> {
        None
    }

    /// Layout node of this component, if it participates in the layout engine.
    ///
    /// Replaces the `[LAYOUT_NODE]()` symbol method of the TS version.
    fn layout_node(&self) -> Option<crate::layout_node::LayoutNode> {
        None
    }
}

/// Components that can take focus and show a hardware cursor.
///
/// While focused the component emits [`CURSOR_MARKER`] at the cursor position;
/// the TUI finds the marker, strips it and places the hardware cursor there
/// (which matters for IME candidate windows).
///
/// Corresponds to `interface Focusable` (`packages/tui/src/tui.ts:57-63`).
pub trait Focusable {
    /// Whether the component currently has focus.
    fn focused(&self) -> bool;
    /// Set the focus state; the component then emits [`CURSOR_MARKER`].
    fn set_focused(&mut self, focused: bool);
}

/// Cursor position marker — an APC (Application Program Command) sequence.
///
/// A zero-width escape sequence terminals ignore. Components emit it at the
/// cursor position while focused; the TUI strips it before output.
pub const CURSOR_MARKER: &str = "\x1b_pi:c\x07";

/// SGR reset plus OSC 8 link close, appended to every non-image line.
/// Crate-visible so components that embed finished child lines into their own
/// styling (`components/box_component.rs`) can strip it first.
pub(crate) const SEGMENT_RESET: &str = "\x1b[0m\x1b]8;;\x07";

/// Minimum interval between two frames.
const MIN_RENDER_INTERVAL_MS: u64 = 16;

/// Result of a [`TuiInputListener`]: consume the input and/or replace it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TuiInputListenerResult {
    /// Stop dispatching this input.
    pub consume: bool,
    /// Replace the input passed to the remaining listeners and the component.
    pub data: Option<String>,
}

/// Listener that can observe, transform or consume terminal input.
pub type TuiInputListener = Box<dyn FnMut(&str) -> Option<TuiInputListenerResult>>;

/// Listener for terminal color scheme changes.
pub type ColorSchemeListener = Box<dyn FnMut(TerminalColorScheme)>;

/// Identifies a registered input listener (TS returns an unsubscribe closure).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListenerId(u64);

/// Container — a component that contains other components.
#[derive(Default)]
pub struct Container {
    /// Child components, rendered in order.
    pub children: Vec<ComponentRef>,
}

impl Container {
    /// Empty container.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a child.
    pub fn add_child(&mut self, component: ComponentRef) {
        self.children.push(component);
    }

    /// Remove a child by identity.
    pub fn remove_child(&mut self, component: &ComponentRef) {
        if let Some(index) = self
            .children
            .iter()
            .position(|child| Rc::ptr_eq(child, component))
        {
            self.children.remove(index);
        }
    }

    /// Drop all children.
    pub fn clear(&mut self) {
        self.children.clear();
    }
}

impl Component for Container {
    fn render(&mut self, width: usize) -> Vec<Line> {
        let mut lines = Vec::new();
        for child in &self.children {
            lines.extend(child.borrow_mut().render(width));
        }
        lines
    }

    fn invalidate(&mut self) {
        for child in &self.children {
            child.borrow_mut().invalidate();
        }
    }

    fn as_container(&self) -> Option<&Container> {
        Some(self)
    }
}

/// Composite overlay content into a terminal line at a fixed column.
pub fn composite_tui_line(
    base_line: &str,
    overlay_line: &str,
    start_col: usize,
    overlay_width: usize,
    total_width: usize,
) -> String {
    if is_image_line(base_line) {
        return base_line.to_string();
    }

    let after_start = start_col + overlay_width;
    let base = extract_segments(
        base_line,
        start_col,
        after_start,
        total_width.saturating_sub(after_start),
        true,
    );
    let overlay = slice_with_width(overlay_line, 0, overlay_width, true);
    let before_pad = start_col.saturating_sub(base.before_width);
    let overlay_pad = overlay_width.saturating_sub(overlay.width);
    let actual_before_width = start_col.max(base.before_width);
    let actual_overlay_width = overlay_width.max(overlay.width);
    let after_target = total_width.saturating_sub(actual_before_width + actual_overlay_width);
    let after_pad = after_target.saturating_sub(base.after_width);
    let result = format!(
        "{}{}{SEGMENT_RESET}{}{}{SEGMENT_RESET}{}{}",
        base.before,
        " ".repeat(before_pad),
        overlay.text,
        " ".repeat(overlay_pad),
        base.after,
        " ".repeat(after_pad)
    );

    if visible_width(&result) <= total_width {
        result
    } else {
        slice_by_column(&result, 0, total_width, true)
    }
}

/// Whether the renderer draws on the main screen or the alternate screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiMode {
    /// Main screen with scrollback.
    Regular,
    /// Alternate screen with a viewport.
    Fullscreen,
}

/// Options for [`Tui::stop`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TuiStopOptions {
    /// Leave renderer output in place for another TUI taking over the terminal.
    pub preserve_screen: bool,
}

/// Anchor position for overlays.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OverlayAnchor {
    /// Centered on both axes.
    #[default]
    Center,
    /// Top left corner.
    TopLeft,
    /// Top right corner.
    TopRight,
    /// Bottom left corner.
    BottomLeft,
    /// Bottom right corner.
    BottomRight,
    /// Top edge, horizontally centered.
    TopCenter,
    /// Bottom edge, horizontally centered.
    BottomCenter,
    /// Left edge, vertically centered.
    LeftCenter,
    /// Right edge, vertically centered.
    RightCenter,
}

/// Margin from the terminal edges.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OverlayMargin {
    /// Top margin (negative values are clamped to zero).
    pub top: i64,
    /// Right margin (negative values are clamped to zero).
    pub right: i64,
    /// Bottom margin (negative values are clamped to zero).
    pub bottom: i64,
    /// Left margin (negative values are clamped to zero).
    pub left: i64,
}

impl OverlayMargin {
    /// Same margin on all sides (TS: `margin: number`).
    pub fn all(value: i64) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }
}

/// A value that is either absolute or a percentage of the reference size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SizeValue {
    /// Absolute number of cells.
    Cells(i64),
    /// Percentage of the reference size, e.g. `SizeValue::Percent(50.0)`.
    Percent(f64),
}

fn parse_size_value(value: Option<SizeValue>, reference_size: usize) -> Option<i64> {
    match value? {
        SizeValue::Cells(cells) => Some(cells),
        SizeValue::Percent(percent) => {
            Some((reference_size as f64 * percent / 100.0).floor() as i64)
        }
    }
}

/// Options for overlay positioning and sizing.
#[derive(Default)]
pub struct OverlayOptions {
    /// Width in columns, or percentage of the terminal width.
    pub width: Option<SizeValue>,
    /// Minimum width in columns.
    pub min_width: Option<usize>,
    /// Maximum height in rows, or percentage of the terminal height.
    pub max_height: Option<SizeValue>,
    /// Anchor point for positioning (default: center).
    pub anchor: Option<OverlayAnchor>,
    /// Horizontal offset from the anchor (positive = right).
    pub offset_x: Option<i64>,
    /// Vertical offset from the anchor (positive = down).
    pub offset_y: Option<i64>,
    /// Row position: absolute, or percentage from the top.
    pub row: Option<SizeValue>,
    /// Column position: absolute, or percentage from the left.
    pub col: Option<SizeValue>,
    /// Margin from the terminal edges.
    pub margin: Option<OverlayMargin>,
    /// Only render the overlay while this returns `true`; called every frame.
    pub visible: Option<Box<dyn Fn(usize, usize) -> bool>>,
    /// If `true`, the overlay does not capture keyboard focus when shown.
    pub non_capturing: bool,
}

impl std::fmt::Debug for OverlayOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OverlayOptions")
            .field("width", &self.width)
            .field("min_width", &self.min_width)
            .field("max_height", &self.max_height)
            .field("anchor", &self.anchor)
            .field("offset_x", &self.offset_x)
            .field("offset_y", &self.offset_y)
            .field("row", &self.row)
            .field("col", &self.col)
            .field("margin", &self.margin)
            .field("visible", &self.visible.is_some())
            .field("non_capturing", &self.non_capturing)
            .finish()
    }
}

struct OverlayEntry {
    id: u64,
    component: ComponentRef,
    options: Option<OverlayOptions>,
    pre_focus: Option<ComponentRef>,
    hidden: bool,
    focus_order: u64,
}

#[derive(Clone)]
enum OverlayBlockedFocusResume {
    RestoreOverlay,
    FocusTarget(Option<ComponentRef>),
}

#[derive(Clone)]
enum OverlayFocusRestoreState {
    Inactive,
    Eligible {
        overlay: u64,
    },
    Blocked {
        overlay: u64,
        blocked_by: ComponentRef,
        resume: OverlayBlockedFocusResume,
    },
}

impl OverlayFocusRestoreState {
    fn overlay_id(&self) -> Option<u64> {
        match self {
            OverlayFocusRestoreState::Inactive => None,
            OverlayFocusRestoreState::Eligible { overlay }
            | OverlayFocusRestoreState::Blocked { overlay, .. } => Some(*overlay),
        }
    }
}

/// Whether `set_focus` clears a pending overlay focus restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverlayFocusRestorePolicy {
    Clear,
    Preserve,
}

struct PendingOsc11Query {
    sender: Option<tokio::sync::oneshot::Sender<Option<RgbColor>>>,
}

struct TuiState {
    terminal: Box<dyn Terminal>,
    children: Vec<ComponentRef>,
    focused: Option<ComponentRef>,
    input_listeners: Vec<(u64, TuiInputListener)>,
    next_listener_id: u64,
    on_debug: Option<Box<dyn FnMut()>>,
    render_requested: bool,
    immediate_render_requested: bool,
    last_render_at: Option<Instant>,
    show_hardware_cursor: bool,
    clear_on_shrink: bool,
    stopped: bool,
    overlay_stack: Vec<OverlayEntry>,
    overlay_focus_restore: OverlayFocusRestoreState,
    focus_order_counter: u64,
    next_overlay_id: u64,
    log_directory: std::path::PathBuf,
    color_scheme_listeners: Vec<(u64, ColorSchemeListener)>,
    next_color_scheme_listener_id: u64,
    color_scheme_notifications_enabled: bool,
    pending_osc11_queries: VecDeque<PendingOsc11Query>,
    cell_size_dirty: bool,
    /// Woken by every render request and by `stop()`, so a loop parked in
    /// [`TuiCore::wait_until_render_due`] re-evaluates its deadline.
    render_notify: Rc<tokio::sync::Notify>,
    /// Focus flags that could not be written because the component was busy
    /// handling input; applied as soon as its borrow is released.
    pending_focus_flags: Vec<(ComponentRef, bool)>,
    /// Set while a component is inside its own `handle_input`, so nested calls
    /// know its borrow is held.
    dispatching_input: bool,
    /// An [`TuiCore::invalidate`] that arrived during that dispatch and runs
    /// as soon as the borrow is released.
    pending_invalidate: bool,
}

/// Shared TUI state — the port of `TuiBase`.
///
/// Cloning yields another handle to the same state, which is how overlay
/// handles mutate the TUI the way the TS closures capture `this`.
#[derive(Clone)]
pub struct TuiCore(Rc<RefCell<TuiState>>);

impl TuiCore {
    /// New core around `terminal`.
    pub fn new(terminal: Box<dyn Terminal>) -> Self {
        Self::with_options(terminal, None, None)
    }

    /// New core with the optional constructor arguments of the TS version
    /// (`showHardwareCursor`, `logDirectory`).
    pub fn with_options(
        terminal: Box<dyn Terminal>,
        show_hardware_cursor: Option<bool>,
        log_directory: Option<std::path::PathBuf>,
    ) -> Self {
        let log_directory = log_directory.unwrap_or_else(|| {
            std::env::var("NOTAGENT_CODING_AGENT_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| {
                    let home = std::env::var("HOME").unwrap_or_default();
                    std::path::PathBuf::from(home)
                        .join(".notagent")
                        .join("agent")
                })
        });
        Self(Rc::new(RefCell::new(TuiState {
            terminal,
            children: Vec::new(),
            focused: None,
            input_listeners: Vec::new(),
            next_listener_id: 0,
            on_debug: None,
            render_requested: false,
            immediate_render_requested: false,
            last_render_at: None,
            show_hardware_cursor: show_hardware_cursor
                .unwrap_or_else(|| std::env::var("NOTAGENT_HARDWARE_CURSOR").as_deref() == Ok("1")),
            clear_on_shrink: std::env::var("NOTAGENT_CLEAR_ON_SHRINK").as_deref() == Ok("1"),
            stopped: false,
            overlay_stack: Vec::new(),
            overlay_focus_restore: OverlayFocusRestoreState::Inactive,
            focus_order_counter: 0,
            next_overlay_id: 0,
            log_directory,
            color_scheme_listeners: Vec::new(),
            next_color_scheme_listener_id: 0,
            color_scheme_notifications_enabled: false,
            pending_osc11_queries: VecDeque::new(),
            cell_size_dirty: false,
            render_notify: Rc::new(tokio::sync::Notify::new()),
            pending_focus_flags: Vec::new(),
            dispatching_input: false,
            pending_invalidate: false,
        })))
    }

    /// Directory for crash and debug logs.
    pub fn log_directory(&self) -> std::path::PathBuf {
        self.0.borrow().log_directory.clone()
    }

    /// Run `f` with the terminal.
    pub fn with_terminal<R>(&self, f: impl FnOnce(&mut dyn Terminal) -> R) -> R {
        let mut state = self.0.borrow_mut();
        f(state.terminal.as_mut())
    }

    /// Terminal width in columns.
    pub fn columns(&self) -> usize {
        self.0.borrow().terminal.columns()
    }

    /// Terminal height in rows.
    pub fn rows(&self) -> usize {
        self.0.borrow().terminal.rows()
    }

    /// Whether `stop()` has been called.
    pub fn is_stopped(&self) -> bool {
        self.0.borrow().stopped
    }

    /// Mounted root components.
    pub fn children(&self) -> Vec<ComponentRef> {
        self.0.borrow().children.clone()
    }

    /// Append a root component.
    pub fn add_child(&self, component: ComponentRef) {
        self.0.borrow_mut().children.push(component);
    }

    /// Remove a root component.
    pub fn remove_child(&self, component: &ComponentRef) {
        let mut state = self.0.borrow_mut();
        if let Some(index) = state
            .children
            .iter()
            .position(|child| Rc::ptr_eq(child, component))
        {
            state.children.remove(index);
        }
    }

    /// Drop all root components.
    pub fn clear(&self) {
        self.0.borrow_mut().children.clear();
    }

    /// Whether the hardware cursor is shown.
    pub fn get_show_hardware_cursor(&self) -> bool {
        self.0.borrow().show_hardware_cursor
    }

    /// Show or hide the hardware cursor.
    pub fn set_show_hardware_cursor(&self, enabled: bool) {
        {
            let mut state = self.0.borrow_mut();
            if state.show_hardware_cursor == enabled {
                return;
            }
            state.show_hardware_cursor = enabled;
            if !enabled {
                state.terminal.hide_cursor();
            }
        }
        self.request_render();
    }

    /// Whether shrinking content triggers a full redraw.
    pub fn get_clear_on_shrink(&self) -> bool {
        self.0.borrow().clear_on_shrink
    }

    /// Set whether shrinking content triggers a full redraw.
    ///
    /// When true, empty rows are cleared when content shrinks; when false they
    /// remain (fewer redraws on slow terminals).
    pub fn set_clear_on_shrink(&self, enabled: bool) {
        self.0.borrow_mut().clear_on_shrink = enabled;
    }

    /// Set the debug key (Shift+Ctrl+D) callback.
    pub fn set_on_debug(&self, on_debug: Option<Box<dyn FnMut()>>) {
        self.0.borrow_mut().on_debug = on_debug;
    }

    /// The focused component, if any.
    pub fn get_focused_component(&self) -> Option<ComponentRef> {
        self.0.borrow().focused.clone()
    }

    /// Move focus to `component`.
    pub fn set_focus(&self, component: Option<ComponentRef>) {
        self.set_focus_internal(component, OverlayFocusRestorePolicy::Clear);
    }

    fn set_focus_internal(
        &self,
        component: Option<ComponentRef>,
        overlay_focus_restore: OverlayFocusRestorePolicy,
    ) {
        let previous_focus = self.0.borrow().focused.clone();
        let mut next_focus = component;

        let previous_focused_overlay = previous_focus
            .as_ref()
            .and_then(|previous| self.visible_overlay_id_for(previous));
        let next_focus_is_overlay = next_focus
            .as_ref()
            .is_some_and(|next| self.overlay_id_for(next).is_some());
        let restore_state = self.get_visible_overlay_focus_restore();

        if next_focus.is_some() && !next_focus_is_overlay {
            let next = next_focus.clone().expect("next focus is some");
            match &restore_state {
                OverlayFocusRestoreState::Blocked {
                    overlay,
                    blocked_by,
                    resume,
                } if previous_focus
                    .as_ref()
                    .is_some_and(|previous| Rc::ptr_eq(previous, blocked_by)) =>
                {
                    let blocked_by_mounted = self.is_component_mounted(blocked_by);
                    if matches!(resume, OverlayBlockedFocusResume::FocusTarget(_))
                        || !blocked_by_mounted
                    {
                        next_focus = self.resolve_blocked_overlay_focus_resume(*overlay, resume);
                    } else {
                        self.0.borrow_mut().overlay_focus_restore =
                            OverlayFocusRestoreState::Blocked {
                                overlay: *overlay,
                                blocked_by: next.clone(),
                                resume: resume.clone(),
                            };
                    }
                }
                _ => {
                    if let Some(previous_overlay) = previous_focused_overlay
                        && restore_state.overlay_id() == Some(previous_overlay)
                        && !self.is_overlay_focus_ancestor(previous_overlay, &next)
                    {
                        self.0.borrow_mut().overlay_focus_restore =
                            OverlayFocusRestoreState::Blocked {
                                overlay: previous_overlay,
                                blocked_by: next.clone(),
                                resume: OverlayBlockedFocusResume::RestoreOverlay,
                            };
                    }
                }
            }
        } else if next_focus.is_none() {
            match &restore_state {
                OverlayFocusRestoreState::Blocked {
                    overlay,
                    blocked_by,
                    resume,
                } if previous_focus
                    .as_ref()
                    .is_some_and(|previous| Rc::ptr_eq(previous, blocked_by)) =>
                {
                    next_focus = self.resolve_blocked_overlay_focus_resume(*overlay, resume);
                }
                _ => {
                    if overlay_focus_restore == OverlayFocusRestorePolicy::Clear {
                        self.clear_overlay_focus_restore();
                    }
                }
            }
        }

        if let Some(previous) = &previous_focus {
            self.write_focus_flag(previous, false);
        }

        self.0.borrow_mut().focused = next_focus.clone();

        if let Some(next) = &next_focus {
            self.write_focus_flag(next, true);
        }

        if let Some(next) = &next_focus
            && let Some(overlay) = self.visible_overlay_id_for(next)
        {
            self.0.borrow_mut().overlay_focus_restore =
                OverlayFocusRestoreState::Eligible { overlay };
        }
    }

    /// Write a component's focus flag.
    ///
    /// A component can call `set_focus` from inside its own `handle_input`; it
    /// is then mutably borrowed and the flag is queued instead (deviation class
    /// 1 — TS has no borrow rules, so the write is applied as soon as the
    /// component returns, before anything can observe it).
    fn write_focus_flag(&self, component: &ComponentRef, focused: bool) {
        match component.try_borrow_mut() {
            Ok(mut borrowed) => {
                if let Some(focusable) = borrowed.as_focusable() {
                    focusable.set_focused(focused);
                }
            }
            Err(_) => self
                .0
                .borrow_mut()
                .pending_focus_flags
                .push((component.clone(), focused)),
        }
    }

    /// Run an [`Self::invalidate`] that was queued while a component was
    /// handling input.
    fn flush_pending_invalidate(&self) {
        if !std::mem::take(&mut self.0.borrow_mut().pending_invalidate) {
            return;
        }
        self.invalidate();
    }

    /// Apply focus flags queued while a component was handling input.
    fn flush_pending_focus_flags(&self) {
        loop {
            let pending = std::mem::take(&mut self.0.borrow_mut().pending_focus_flags);
            if pending.is_empty() {
                return;
            }
            for (component, focused) in pending {
                if let Ok(mut borrowed) = component.try_borrow_mut()
                    && let Some(focusable) = borrowed.as_focusable()
                {
                    focusable.set_focused(focused);
                }
            }
        }
    }

    fn overlay_id_for(&self, component: &ComponentRef) -> Option<u64> {
        self.0
            .borrow()
            .overlay_stack
            .iter()
            .find(|entry| Rc::ptr_eq(&entry.component, component))
            .map(|entry| entry.id)
    }

    fn visible_overlay_id_for(&self, component: &ComponentRef) -> Option<u64> {
        let ids: Vec<u64> = self
            .0
            .borrow()
            .overlay_stack
            .iter()
            .filter(|entry| Rc::ptr_eq(&entry.component, component))
            .map(|entry| entry.id)
            .collect();
        ids.into_iter().find(|id| self.is_overlay_visible(*id))
    }

    fn clear_overlay_focus_restore(&self) {
        self.0.borrow_mut().overlay_focus_restore = OverlayFocusRestoreState::Inactive;
    }

    fn clear_overlay_focus_restore_for(&self, overlay: u64) {
        let mut state = self.0.borrow_mut();
        if state.overlay_focus_restore.overlay_id() == Some(overlay) {
            state.overlay_focus_restore = OverlayFocusRestoreState::Inactive;
        }
    }

    fn resolve_blocked_overlay_focus_resume(
        &self,
        overlay: u64,
        resume: &OverlayBlockedFocusResume,
    ) -> Option<ComponentRef> {
        match resume {
            OverlayBlockedFocusResume::RestoreOverlay => self.overlay_component(overlay),
            OverlayBlockedFocusResume::FocusTarget(target) => {
                self.clear_overlay_focus_restore();
                target.clone()
            }
        }
    }

    fn overlay_component(&self, overlay: u64) -> Option<ComponentRef> {
        self.0
            .borrow()
            .overlay_stack
            .iter()
            .find(|entry| entry.id == overlay)
            .map(|entry| entry.component.clone())
    }

    fn get_visible_overlay_focus_restore(&self) -> OverlayFocusRestoreState {
        let restore_state = self.0.borrow().overlay_focus_restore.clone();
        let Some(overlay) = restore_state.overlay_id() else {
            return restore_state;
        };
        let exists = self
            .0
            .borrow()
            .overlay_stack
            .iter()
            .any(|entry| entry.id == overlay);
        if !exists || !self.is_overlay_visible(overlay) {
            return OverlayFocusRestoreState::Inactive;
        }
        restore_state
    }

    fn is_overlay_focus_ancestor(&self, overlay: u64, component: &ComponentRef) -> bool {
        let mut visited: Vec<ComponentRef> = Vec::new();
        let mut current = self
            .0
            .borrow()
            .overlay_stack
            .iter()
            .find(|entry| entry.id == overlay)
            .and_then(|entry| entry.pre_focus.clone());
        while let Some(node) = current {
            if visited.iter().any(|seen| Rc::ptr_eq(seen, &node)) {
                break;
            }
            visited.push(node.clone());
            if Rc::ptr_eq(&node, component) {
                return true;
            }
            current = self
                .0
                .borrow()
                .overlay_stack
                .iter()
                .find(|entry| Rc::ptr_eq(&entry.component, &node))
                .and_then(|entry| entry.pre_focus.clone());
        }
        false
    }

    fn retarget_overlay_pre_focus(&self, removed_id: u64) {
        let mut state = self.0.borrow_mut();
        let Some(removed) = state.overlay_stack.iter().find(|e| e.id == removed_id) else {
            return;
        };
        let removed_component = removed.component.clone();
        let removed_pre_focus = removed.pre_focus.clone();
        for overlay in &mut state.overlay_stack {
            if overlay.id != removed_id
                && overlay
                    .pre_focus
                    .as_ref()
                    .is_some_and(|pre| Rc::ptr_eq(pre, &removed_component))
            {
                overlay.pre_focus = removed_pre_focus.clone();
            }
        }
    }

    /// Root components the focus machine considers mounted.
    ///
    /// The alternate screen renderer overrides this with its layout root; the
    /// main screen uses the children.
    fn is_component_mounted(&self, component: &ComponentRef) -> bool {
        self.0
            .borrow()
            .children
            .iter()
            .any(|child| contains_component(child, component))
    }

    /// Show an overlay with configurable positioning and sizing.
    pub fn show_overlay(
        &self,
        component: ComponentRef,
        options: Option<OverlayOptions>,
    ) -> OverlayHandle {
        let non_capturing = options.as_ref().is_some_and(|o| o.non_capturing);
        let (id, focus_order) = {
            let mut state = self.0.borrow_mut();
            state.next_overlay_id += 1;
            state.focus_order_counter += 1;
            let id = state.next_overlay_id;
            let focus_order = state.focus_order_counter;
            let pre_focus = state.focused.clone();
            state.overlay_stack.push(OverlayEntry {
                id,
                component: component.clone(),
                options,
                pre_focus,
                hidden: false,
                focus_order,
            });
            (id, focus_order)
        };
        let _ = focus_order;

        // Only focus if the overlay is actually visible.
        if !non_capturing && self.is_overlay_visible(id) {
            self.set_focus(Some(component.clone()));
        }
        self.0.borrow_mut().terminal.hide_cursor();
        self.request_render();

        OverlayHandle {
            core: self.clone(),
            id,
            component,
        }
    }

    /// Hide the topmost overlay and restore the previous focus.
    pub fn hide_overlay(&self) {
        let Some((id, component, pre_focus)) = self
            .0
            .borrow()
            .overlay_stack
            .last()
            .map(|entry| (entry.id, entry.component.clone(), entry.pre_focus.clone()))
        else {
            return;
        };
        self.clear_overlay_focus_restore_for(id);
        self.retarget_overlay_pre_focus(id);
        self.0.borrow_mut().overlay_stack.pop();

        let focused_is_overlay = self
            .0
            .borrow()
            .focused
            .as_ref()
            .is_some_and(|focused| Rc::ptr_eq(focused, &component));
        if focused_is_overlay {
            let top_visible = self.get_topmost_visible_overlay();
            self.set_focus(top_visible.or(pre_focus));
        }
        if self.0.borrow().overlay_stack.is_empty() {
            self.0.borrow_mut().terminal.hide_cursor();
        }
        self.request_render();
    }

    /// Whether any overlay is currently visible.
    pub fn has_overlay(&self) -> bool {
        let ids: Vec<u64> = self
            .0
            .borrow()
            .overlay_stack
            .iter()
            .map(|entry| entry.id)
            .collect();
        ids.into_iter().any(|id| self.is_overlay_visible(id))
    }

    /// Whether the overlay stack has any entries (visible or not).
    pub fn has_overlay_entries(&self) -> bool {
        !self.0.borrow().overlay_stack.is_empty()
    }

    fn is_overlay_visible(&self, id: u64) -> bool {
        let (hidden, visible_fn_present, columns, rows) = {
            let state = self.0.borrow();
            let Some(entry) = state.overlay_stack.iter().find(|entry| entry.id == id) else {
                return false;
            };
            if entry.hidden {
                return false;
            }
            let has_visible = entry
                .options
                .as_ref()
                .is_some_and(|options| options.visible.is_some());
            (
                entry.hidden,
                has_visible,
                state.terminal.columns(),
                state.terminal.rows(),
            )
        };
        if hidden {
            return false;
        }
        if !visible_fn_present {
            return true;
        }
        let state = self.0.borrow();
        let entry = state
            .overlay_stack
            .iter()
            .find(|entry| entry.id == id)
            .expect("overlay exists");
        let visible = entry
            .options
            .as_ref()
            .and_then(|options| options.visible.as_ref())
            .expect("visible callback present");
        visible(columns, rows)
    }

    fn get_topmost_visible_overlay(&self) -> Option<ComponentRef> {
        let candidates: Vec<(u64, u64, bool)> = self
            .0
            .borrow()
            .overlay_stack
            .iter()
            .map(|entry| {
                (
                    entry.id,
                    entry.focus_order,
                    entry.options.as_ref().is_some_and(|o| o.non_capturing),
                )
            })
            .collect();
        let mut topmost: Option<(u64, u64)> = None;
        for (id, focus_order, non_capturing) in candidates {
            if non_capturing || !self.is_overlay_visible(id) {
                continue;
            }
            if topmost.is_none_or(|(_, top_order)| focus_order > top_order) {
                topmost = Some((id, focus_order));
            }
        }
        topmost.and_then(|(id, _)| self.overlay_component(id))
    }

    /// Invalidate all mounted components and overlays.
    ///
    /// A component can reach this from inside its own `handle_input` — the
    /// theme preview of the settings menu does, through the theme controller
    /// (`theme-controller.ts:82-88`). It is then mutably borrowed and the walk
    /// below would borrow it a second time, so the call is queued and runs the
    /// moment the dispatch returns (deviation class 1, the same treatment as
    /// [`Self::write_focus_flag`]: TS has no borrow rules, and nothing renders
    /// between the two points).
    pub fn invalidate(&self) {
        if self.0.borrow().dispatching_input {
            self.0.borrow_mut().pending_invalidate = true;
            return;
        }
        let roots = self.0.borrow().children.clone();
        for root in roots {
            root.borrow_mut().invalidate();
        }
        let overlays: Vec<ComponentRef> = self
            .0
            .borrow()
            .overlay_stack
            .iter()
            .map(|entry| entry.component.clone())
            .collect();
        for overlay in overlays {
            overlay.borrow_mut().invalidate();
        }
    }

    /// Register an input listener; returns its id for removal.
    pub fn add_input_listener(&self, listener: TuiInputListener) -> ListenerId {
        let mut state = self.0.borrow_mut();
        state.next_listener_id += 1;
        let id = state.next_listener_id;
        state.input_listeners.push((id, listener));
        ListenerId(id)
    }

    /// Remove a previously registered input listener.
    pub fn remove_input_listener(&self, id: ListenerId) {
        self.0
            .borrow_mut()
            .input_listeners
            .retain(|(listener_id, _)| *listener_id != id.0);
    }

    /// Register a color scheme listener; returns its id for removal.
    pub fn on_terminal_color_scheme_change(&self, listener: ColorSchemeListener) -> ListenerId {
        let mut state = self.0.borrow_mut();
        state.next_color_scheme_listener_id += 1;
        let id = state.next_color_scheme_listener_id;
        state.color_scheme_listeners.push((id, listener));
        ListenerId(id)
    }

    /// Remove a color scheme listener.
    pub fn remove_terminal_color_scheme_listener(&self, id: ListenerId) {
        self.0
            .borrow_mut()
            .color_scheme_listeners
            .retain(|(listener_id, _)| *listener_id != id.0);
    }

    /// Enable or disable color palette change notifications (DECSET 2031).
    pub fn set_terminal_color_scheme_notifications(&self, enabled: bool) {
        let mut state = self.0.borrow_mut();
        if state.color_scheme_notifications_enabled == enabled {
            return;
        }
        state.color_scheme_notifications_enabled = enabled;
        if !state.stopped {
            let sequence = if enabled {
                "\x1b[?2031h"
            } else {
                "\x1b[?2031l"
            };
            state.terminal.write(sequence);
        }
    }

    /// Start the terminal and the render loop bookkeeping.
    pub fn start(&self, mut on_input: Box<dyn FnMut(&str)>) {
        {
            let mut state = self.0.borrow_mut();
            state.stopped = false;
        }
        let core = self.clone();
        let core_resize = self.clone();
        self.0.borrow_mut().terminal.start(
            Box::new(move |data| on_input(data)),
            Box::new(move || core_resize.request_render()),
        );
        let _ = core;
        {
            let mut state = self.0.borrow_mut();
            state.terminal.hide_cursor();
            if state.color_scheme_notifications_enabled {
                state.terminal.write("\x1b[?2031h");
            }
        }
        self.query_cell_size();
        self.request_render();
    }

    fn query_cell_size(&self) {
        // Only query if the terminal supports images (cell size is only used for
        // image rendering).
        if get_capabilities().images.is_none() {
            return;
        }
        // Query terminal for cell size in pixels: CSI 16 t
        // Response format: CSI 6 ; height ; width t
        self.0.borrow_mut().terminal.write("\x1b[16t");
    }

    /// Stop the terminal.
    pub fn stop(&self) {
        let mut state = self.0.borrow_mut();
        state.stopped = true;
        state.render_requested = false;
        state.immediate_render_requested = false;
        state.render_notify.notify_waiters();
        if state.color_scheme_notifications_enabled {
            state.terminal.write("\x1b[?2031l");
        }
        state.terminal.show_cursor();
        state.terminal.stop();
    }

    /// Request a frame (throttled to 16 ms).
    pub fn request_render(&self) {
        let notify = {
            let mut state = self.0.borrow_mut();
            state.render_requested = true;
            Rc::clone(&state.render_notify)
        };
        notify.notify_waiters();
    }

    /// Request a frame that preempts the throttle (used after key input).
    pub fn request_immediate_render(&self) {
        let notify = {
            let mut state = self.0.borrow_mut();
            state.render_requested = true;
            state.immediate_render_requested = true;
            Rc::clone(&state.render_notify)
        };
        notify.notify_waiters();
    }

    /// Wait until the next frame is due.
    ///
    /// The render loop's counterpart to [`Self::render_deadline`]: it stays
    /// pending while nothing is requested and wakes as soon as a request comes
    /// in, which is what `scheduleRender()`'s `setTimeout` does on the Node
    /// event loop (`packages/tui/src/tui.ts:243-258`). Cancel-safe: dropping the
    /// future keeps the request, the next call recomputes the deadline.
    pub async fn wait_until_render_due(&self) {
        loop {
            let notify = Rc::clone(&self.0.borrow().render_notify);
            let notified = notify.notified();
            tokio::pin!(notified);
            // Register before reading the deadline, otherwise a request between
            // the two is lost and the loop sleeps through its frame.
            notified.as_mut().enable();

            let Some(deadline) = self.render_deadline() else {
                notified.await;
                continue;
            };
            let now = Instant::now();
            if deadline <= now {
                return;
            }
            tokio::select! {
                () = tokio::time::sleep(deadline - now) => return,
                () = notified => continue,
            }
        }
    }

    /// When the next frame is due, or `None` if none is pending.
    ///
    /// Replaces `scheduleRender()`: `MIN_RENDER_INTERVAL_MS` after the previous
    /// frame, or immediately after keyboard input.
    pub fn render_deadline(&self) -> Option<Instant> {
        let state = self.0.borrow();
        if state.stopped || !state.render_requested {
            return None;
        }
        if state.immediate_render_requested {
            return Some(Instant::now());
        }
        let Some(last_render_at) = state.last_render_at else {
            return Some(Instant::now());
        };
        Some(last_render_at + Duration::from_millis(MIN_RENDER_INTERVAL_MS))
    }

    /// Mark a frame as rendered; returns `false` when nothing was pending.
    pub fn begin_frame(&self) -> bool {
        let mut state = self.0.borrow_mut();
        state.render_requested = false;
        state.immediate_render_requested = false;
        state.last_render_at = Some(Instant::now());
        !state.stopped
    }

    /// Dispatch terminal input (`TuiBase.handleTerminalInput`).
    ///
    /// Order: OSC 11 reply → color scheme report → input listeners → cell size →
    /// debug key → overlay focus repair → focused component.
    pub fn handle_terminal_input(&self, data: &str) {
        if self.consume_osc11_background_response(data) {
            return;
        }
        if self.consume_terminal_color_scheme_report(data) {
            return;
        }

        let mut data = data.to_string();
        let listener_count = self.0.borrow().input_listeners.len();
        if listener_count > 0 {
            let mut current = data.clone();
            for index in 0..listener_count {
                let mut listener = {
                    let mut state = self.0.borrow_mut();
                    if index >= state.input_listeners.len() {
                        break;
                    }
                    state.input_listeners.remove(index)
                };
                let result = (listener.1)(&current);
                {
                    let mut state = self.0.borrow_mut();
                    let position = index.min(state.input_listeners.len());
                    state.input_listeners.insert(position, listener);
                }
                if let Some(result) = result {
                    if result.consume {
                        return;
                    }
                    if let Some(replacement) = result.data {
                        current = replacement;
                    }
                }
            }
            if current.is_empty() {
                return;
            }
            data = current;
        }

        // Consume terminal cell size responses without blocking unrelated input.
        if self.consume_cell_size_response(&data) {
            return;
        }

        // Global debug key handler (Shift+Ctrl+D).
        if matches_key(&data, "shift+ctrl+d") {
            let mut on_debug = self.0.borrow_mut().on_debug.take();
            if let Some(handler) = on_debug.as_mut() {
                handler();
                self.0.borrow_mut().on_debug = on_debug;
                return;
            }
            self.0.borrow_mut().on_debug = on_debug;
        }

        self.repair_overlay_focus();

        // Pass input to the focused component (including Ctrl+C); the component
        // decides how to handle it.
        let focused = self.0.borrow().focused.clone();
        if let Some(focused) = focused {
            let wants_key_release = focused.borrow().wants_key_release();
            if is_key_release(&data) && !wants_key_release {
                return;
            }
            self.0.borrow_mut().dispatching_input = true;
            focused.borrow_mut().handle_input(&data);
            self.0.borrow_mut().dispatching_input = false;
            self.flush_pending_focus_flags();
            self.flush_pending_invalidate();
            // Keyboard input is latency sensitive: skip the throttled path.
            self.request_immediate_render();
        }
    }

    /// If the focused overlay became invisible, move focus to the topmost
    /// visible one; then apply a pending overlay focus restore.
    fn repair_overlay_focus(&self) {
        let focused = self.0.borrow().focused.clone();
        if let Some(focused) = &focused
            && let Some(overlay) = self.overlay_id_for(focused)
            && !self.is_overlay_visible(overlay)
        {
            match self.get_topmost_visible_overlay() {
                Some(top_visible) => self.set_focus(Some(top_visible)),
                None => {
                    let pre_focus = self
                        .0
                        .borrow()
                        .overlay_stack
                        .iter()
                        .find(|entry| entry.id == overlay)
                        .and_then(|entry| entry.pre_focus.clone());
                    self.set_focus_internal(pre_focus, OverlayFocusRestorePolicy::Preserve);
                }
            }
        }

        let focused = self.0.borrow().focused.clone();
        let focus_is_overlay = focused
            .as_ref()
            .is_some_and(|focused| self.overlay_id_for(focused).is_some());
        if focus_is_overlay {
            return;
        }
        match self.get_visible_overlay_focus_restore() {
            OverlayFocusRestoreState::Eligible { overlay } => {
                self.set_focus(self.overlay_component(overlay));
            }
            OverlayFocusRestoreState::Blocked {
                overlay,
                blocked_by,
                resume,
            } if !focused
                .as_ref()
                .is_some_and(|focused| Rc::ptr_eq(focused, &blocked_by)) =>
            {
                match resume {
                    OverlayBlockedFocusResume::RestoreOverlay => {
                        self.set_focus(self.overlay_component(overlay));
                    }
                    OverlayBlockedFocusResume::FocusTarget(target) => {
                        self.clear_overlay_focus_restore();
                        self.set_focus(target);
                    }
                }
            }
            _ => {}
        }
    }

    fn consume_osc11_background_response(&self, data: &str) -> bool {
        if self.0.borrow().pending_osc11_queries.is_empty() {
            return false;
        }
        if !is_osc11_background_color_response(data) {
            return false;
        }
        let rgb = parse_osc11_background_color(data);
        let query = self.0.borrow_mut().pending_osc11_queries.pop_front();
        if let Some(mut query) = query
            && let Some(sender) = query.sender.take()
        {
            let _ = sender.send(rgb);
        }
        true
    }

    fn consume_terminal_color_scheme_report(&self, data: &str) -> bool {
        let Some(scheme) = parse_terminal_color_scheme_report(data) else {
            return false;
        };
        let count = self.0.borrow().color_scheme_listeners.len();
        for index in 0..count {
            let mut listener = {
                let mut state = self.0.borrow_mut();
                if index >= state.color_scheme_listeners.len() {
                    break;
                }
                state.color_scheme_listeners.remove(index)
            };
            (listener.1)(scheme);
            let mut state = self.0.borrow_mut();
            let position = index.min(state.color_scheme_listeners.len());
            state.color_scheme_listeners.insert(position, listener);
        }
        true
    }

    fn consume_cell_size_response(&self, data: &str) -> bool {
        // Response format: ESC [ 6 ; height ; width t
        let Some(body) = data
            .strip_prefix("\x1b[6;")
            .and_then(|rest| rest.strip_suffix('t'))
        else {
            return false;
        };
        let mut parts = body.split(';');
        let (Some(height), Some(width), None) = (parts.next(), parts.next(), parts.next()) else {
            return false;
        };
        let (Ok(height_px), Ok(width_px)) = (height.parse::<i64>(), width.parse::<i64>()) else {
            return false;
        };
        if height_px <= 0 || width_px <= 0 {
            return true;
        }
        set_cell_dimensions(CellDimensions {
            width_px: width_px as u32,
            height_px: height_px as u32,
        });
        // Invalidate all components so images re-render with correct dimensions.
        self.invalidate();
        self.0.borrow_mut().cell_size_dirty = true;
        self.request_render();
        true
    }

    /// Query the terminal's default background color with OSC 11.
    pub async fn query_terminal_background_color(&self, timeout: Duration) -> Option<RgbColor> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        {
            let mut state = self.0.borrow_mut();
            state.pending_osc11_queries.push_back(PendingOsc11Query {
                sender: Some(sender),
            });
            state.terminal.write("\x1b]11;?\x07");
        }
        match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(rgb)) => rgb,
            _ => None,
        }
    }

    /// Query the terminal's color-scheme preference with DSR (`CSI ? 996 n`).
    ///
    /// Terminals supporting the color palette notification protocol reply with
    /// `CSI ? 997 ; 1 n` for dark or `CSI ? 997 ; 2 n` for light.
    pub async fn query_terminal_color_scheme(
        &self,
        timeout: Duration,
    ) -> Option<TerminalColorScheme> {
        let received: Rc<RefCell<Option<TerminalColorScheme>>> = Rc::new(RefCell::new(None));
        let sink = Rc::clone(&received);
        let id = self.on_terminal_color_scheme_change(Box::new(move |scheme| {
            *sink.borrow_mut() = Some(scheme);
        }));
        self.0.borrow_mut().terminal.write("\x1b[?996n");

        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(scheme) = *received.borrow() {
                self.remove_terminal_color_scheme_listener(id);
                return Some(scheme);
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        self.remove_terminal_color_scheme_listener(id);
        *received.borrow()
    }

    /// Composite all overlays into the content lines (sorted by focus order,
    /// higher = on top).
    pub fn composite_overlays(
        &self,
        lines: Vec<Line>,
        term_width: usize,
        term_height: usize,
    ) -> Vec<Line> {
        if self.0.borrow().overlay_stack.is_empty() {
            return lines;
        }
        let mut result = lines;

        // Pre-render all visible overlays and calculate positions.
        struct Rendered {
            overlay_lines: Vec<Line>,
            row: usize,
            col: usize,
            width: usize,
        }
        let mut rendered: Vec<Rendered> = Vec::new();
        let mut min_lines_needed = result.len();

        let mut visible_entries: Vec<(u64, u64)> = self
            .0
            .borrow()
            .overlay_stack
            .iter()
            .map(|entry| (entry.id, entry.focus_order))
            .collect();
        visible_entries.retain(|(id, _)| self.is_overlay_visible(*id));
        visible_entries.sort_by_key(|(_, focus_order)| *focus_order);

        for (id, _) in visible_entries {
            let component = self.overlay_component(id).expect("overlay exists");

            // Layout with height 0 first: width and maxHeight do not depend on
            // the overlay height.
            let layout = self.resolve_overlay_layout(id, 0, term_width, term_height);
            let mut overlay_lines = component.borrow_mut().render(layout.width);

            if let Some(max_height) = layout.max_height
                && overlay_lines.len() > max_height
            {
                overlay_lines.truncate(max_height);
            }

            let final_layout =
                self.resolve_overlay_layout(id, overlay_lines.len(), term_width, term_height);
            min_lines_needed = min_lines_needed.max(final_layout.row + overlay_lines.len());
            rendered.push(Rendered {
                overlay_lines,
                row: final_layout.row,
                col: final_layout.col,
                width: layout.width,
            });
        }

        // Pad to at least terminal height so overlays have screen-relative
        // positions. Deliberately excludes maxLinesRendered: the historical
        // high-water mark caused self-reinforcing inflation that pushed content
        // into scrollback when the terminal was widened.
        let working_height = result.len().max(term_height).max(min_lines_needed);
        while result.len() < working_height {
            result.push(Line::from(""));
        }
        let viewport_start = working_height.saturating_sub(term_height);

        for item in rendered {
            for (index, overlay_line) in item.overlay_lines.iter().enumerate() {
                let idx = viewport_start + item.row + index;
                if idx < result.len() {
                    // Defensive: truncate the overlay line to its declared width.
                    let truncated: Line = if visible_width(overlay_line) > item.width {
                        Line::from(slice_by_column(overlay_line, 0, item.width, true))
                    } else {
                        overlay_line.clone()
                    };
                    result[idx] = Line::from(composite_tui_line(
                        &result[idx],
                        &truncated,
                        item.col,
                        item.width,
                        term_width,
                    ));
                }
            }
        }

        result
    }

    /// Normalize every non-image line and append the segment reset.
    ///
    /// Idempotent: a line that already ends in the reset is left untouched, so
    /// a component may finish its lines while filling its cache
    /// ([`finish_line`]) and this pass stays a no-op for them. Image lines are
    /// exempt via `is_image_line`, same as the TS original (`tui.ts:1157`).
    pub fn apply_line_resets(&self, lines: &mut [Line]) {
        for line in lines.iter_mut() {
            if line.ends_with(SEGMENT_RESET) || is_image_line(line) {
                continue;
            }
            *line = Line::from(finish_line(line));
        }
    }

    /// Find the cursor marker, compute its position and strip it.
    ///
    /// Only the bottom `height` lines (the visible viewport) are scanned.
    pub fn extract_cursor_position(
        &self,
        lines: &mut [Line],
        height: usize,
    ) -> Option<(usize, usize)> {
        let viewport_top = lines.len().saturating_sub(height);
        for row in (viewport_top..lines.len()).rev() {
            if let Some(marker_index) = lines[row].find(CURSOR_MARKER) {
                let col = visible_width(&lines[row][..marker_index]);
                lines[row] = Line::from(format!(
                    "{}{}",
                    &lines[row][..marker_index],
                    &lines[row][marker_index + CURSOR_MARKER.len()..]
                ));
                return Some((row, col));
            }
        }
        None
    }

    fn resolve_overlay_layout(
        &self,
        id: u64,
        overlay_height: usize,
        term_width: usize,
        term_height: usize,
    ) -> ResolvedOverlayLayout {
        let state = self.0.borrow();
        let entry = state
            .overlay_stack
            .iter()
            .find(|entry| entry.id == id)
            .expect("overlay exists");
        let options = entry.options.as_ref();

        let margin = options.and_then(|o| o.margin).unwrap_or_default();
        let (margin_top, margin_right, margin_bottom, margin_left) = (
            margin.top.max(0) as usize,
            margin.right.max(0) as usize,
            margin.bottom.max(0) as usize,
            margin.left.max(0) as usize,
        );

        let avail_width = term_width.saturating_sub(margin_left + margin_right).max(1);
        let avail_height = term_height
            .saturating_sub(margin_top + margin_bottom)
            .max(1);

        // === width ===
        let mut width = parse_size_value(options.and_then(|o| o.width), term_width)
            .unwrap_or_else(|| 80.min(avail_width) as i64);
        if let Some(min_width) = options.and_then(|o| o.min_width) {
            width = width.max(min_width as i64);
        }
        let width = width.clamp(1, avail_width as i64) as usize;

        // === maxHeight ===
        let max_height = parse_size_value(options.and_then(|o| o.max_height), term_height)
            .map(|value| value.clamp(1, avail_height as i64) as usize);

        let effective_height = max_height.map_or(overlay_height, |max| overlay_height.min(max));

        // === position ===
        let anchor = options.and_then(|o| o.anchor).unwrap_or_default();
        let mut row: i64 = match options.and_then(|o| o.row) {
            Some(SizeValue::Percent(percent)) => {
                let max_row = avail_height.saturating_sub(effective_height) as f64;
                margin_top as i64 + (max_row * percent / 100.0).floor() as i64
            }
            Some(SizeValue::Cells(cells)) => cells,
            None => resolve_anchor_row(anchor, effective_height, avail_height, margin_top),
        };
        let mut col: i64 = match options.and_then(|o| o.col) {
            Some(SizeValue::Percent(percent)) => {
                let max_col = avail_width.saturating_sub(width) as f64;
                margin_left as i64 + (max_col * percent / 100.0).floor() as i64
            }
            Some(SizeValue::Cells(cells)) => cells,
            None => resolve_anchor_col(anchor, width, avail_width, margin_left),
        };

        if let Some(offset_y) = options.and_then(|o| o.offset_y) {
            row += offset_y;
        }
        if let Some(offset_x) = options.and_then(|o| o.offset_x) {
            col += offset_x;
        }

        // Clamp to terminal bounds (respecting margins).
        row = row
            .min(term_height as i64 - margin_bottom as i64 - effective_height as i64)
            .max(margin_top as i64);
        col = col
            .min(term_width as i64 - margin_right as i64 - width as i64)
            .max(margin_left as i64);

        ResolvedOverlayLayout {
            width,
            row: row.max(0) as usize,
            col: col.max(0) as usize,
            max_height,
        }
    }
}

struct ResolvedOverlayLayout {
    width: usize,
    row: usize,
    col: usize,
    max_height: Option<usize>,
}

fn resolve_anchor_row(
    anchor: OverlayAnchor,
    height: usize,
    avail_height: usize,
    margin_top: usize,
) -> i64 {
    match anchor {
        OverlayAnchor::TopLeft | OverlayAnchor::TopCenter | OverlayAnchor::TopRight => {
            margin_top as i64
        }
        OverlayAnchor::BottomLeft | OverlayAnchor::BottomCenter | OverlayAnchor::BottomRight => {
            margin_top as i64 + avail_height as i64 - height as i64
        }
        OverlayAnchor::LeftCenter | OverlayAnchor::Center | OverlayAnchor::RightCenter => {
            margin_top as i64 + ((avail_height as i64 - height as i64) as f64 / 2.0).floor() as i64
        }
    }
}

fn resolve_anchor_col(
    anchor: OverlayAnchor,
    width: usize,
    avail_width: usize,
    margin_left: usize,
) -> i64 {
    match anchor {
        OverlayAnchor::TopLeft | OverlayAnchor::LeftCenter | OverlayAnchor::BottomLeft => {
            margin_left as i64
        }
        OverlayAnchor::TopRight | OverlayAnchor::RightCenter | OverlayAnchor::BottomRight => {
            margin_left as i64 + avail_width as i64 - width as i64
        }
        OverlayAnchor::TopCenter | OverlayAnchor::Center | OverlayAnchor::BottomCenter => {
            margin_left as i64 + ((avail_width as i64 - width as i64) as f64 / 2.0).floor() as i64
        }
    }
}

fn contains_component(root: &ComponentRef, target: &ComponentRef) -> bool {
    if Rc::ptr_eq(root, target) {
        return true;
    }
    // A component that is currently handling input is mutably borrowed; it is
    // not the target (checked above) and cannot be walked into, so treat it as
    // a leaf.
    let Ok(borrowed) = root.try_borrow() else {
        return false;
    };
    let Some(container) = borrowed.as_container() else {
        return false;
    };
    container
        .children
        .iter()
        .any(|child| contains_component(child, target))
}

/// Options for [`OverlayHandle::unfocus_to`].
#[derive(Clone, Default)]
pub struct OverlayUnfocusOptions {
    /// Explicit target to focus after releasing this overlay.
    pub target: Option<ComponentRef>,
}

/// Handle returned by [`TuiCore::show_overlay`].
pub struct OverlayHandle {
    core: TuiCore,
    id: u64,
    component: ComponentRef,
}

impl OverlayHandle {
    /// Permanently remove the overlay (it cannot be shown again).
    pub fn hide(&self) {
        let index = self
            .core
            .0
            .borrow()
            .overlay_stack
            .iter()
            .position(|entry| entry.id == self.id);
        let Some(index) = index else {
            return;
        };
        self.core.clear_overlay_focus_restore_for(self.id);
        self.core.retarget_overlay_pre_focus(self.id);
        let pre_focus = self
            .core
            .0
            .borrow_mut()
            .overlay_stack
            .remove(index)
            .pre_focus;

        let had_focus = self
            .core
            .0
            .borrow()
            .focused
            .as_ref()
            .is_some_and(|focused| Rc::ptr_eq(focused, &self.component));
        if had_focus {
            let top_visible = self.core.get_topmost_visible_overlay();
            self.core.set_focus(top_visible.or(pre_focus));
        }
        if self.core.0.borrow().overlay_stack.is_empty() {
            self.core.0.borrow_mut().terminal.hide_cursor();
        }
        self.core.request_render();
    }

    /// Temporarily hide or show the overlay.
    pub fn set_hidden(&self, hidden: bool) {
        {
            let mut state = self.core.0.borrow_mut();
            let Some(entry) = state.overlay_stack.iter_mut().find(|e| e.id == self.id) else {
                return;
            };
            if entry.hidden == hidden {
                return;
            }
            entry.hidden = hidden;
        }

        if hidden {
            self.core.clear_overlay_focus_restore_for(self.id);
            // If this overlay had focus, move it to the next visible or preFocus.
            let had_focus = self
                .core
                .0
                .borrow()
                .focused
                .as_ref()
                .is_some_and(|focused| Rc::ptr_eq(focused, &self.component));
            if had_focus {
                let top_visible = self.core.get_topmost_visible_overlay();
                let pre_focus = self
                    .core
                    .0
                    .borrow()
                    .overlay_stack
                    .iter()
                    .find(|e| e.id == self.id)
                    .and_then(|entry| entry.pre_focus.clone());
                self.core.set_focus(top_visible.or(pre_focus));
            }
        } else {
            // Restore focus to this overlay when showing (if actually visible).
            let non_capturing = self
                .core
                .0
                .borrow()
                .overlay_stack
                .iter()
                .find(|e| e.id == self.id)
                .and_then(|entry| entry.options.as_ref().map(|o| o.non_capturing))
                .unwrap_or(false);
            if !non_capturing && self.core.is_overlay_visible(self.id) {
                {
                    let mut state = self.core.0.borrow_mut();
                    state.focus_order_counter += 1;
                    let focus_order = state.focus_order_counter;
                    if let Some(entry) = state.overlay_stack.iter_mut().find(|e| e.id == self.id) {
                        entry.focus_order = focus_order;
                    }
                }
                self.core.set_focus(Some(self.component.clone()));
            }
        }
        self.core.request_render();
    }

    /// Whether the overlay is temporarily hidden.
    pub fn is_hidden(&self) -> bool {
        self.core
            .0
            .borrow()
            .overlay_stack
            .iter()
            .find(|entry| entry.id == self.id)
            .is_some_and(|entry| entry.hidden)
    }

    /// Focus this overlay and bring it to the visual front.
    pub fn focus(&self) {
        let exists = self
            .core
            .0
            .borrow()
            .overlay_stack
            .iter()
            .any(|entry| entry.id == self.id);
        if !exists || !self.core.is_overlay_visible(self.id) {
            return;
        }
        {
            let mut state = self.core.0.borrow_mut();
            state.focus_order_counter += 1;
            let focus_order = state.focus_order_counter;
            if let Some(entry) = state.overlay_stack.iter_mut().find(|e| e.id == self.id) {
                entry.focus_order = focus_order;
            }
        }
        self.core.set_focus(Some(self.component.clone()));
        self.core.request_render();
    }

    /// Release focus to the next visible capturing overlay or the previous target.
    pub fn unfocus(&self) {
        self.unfocus_internal(None);
    }

    /// Release focus to an explicit target.
    pub fn unfocus_to(&self, options: OverlayUnfocusOptions) {
        self.unfocus_internal(Some(options));
    }

    fn unfocus_internal(&self, options: Option<OverlayUnfocusOptions>) {
        let is_focused = self.is_focused();
        let restore_state = self.core.0.borrow().overlay_focus_restore.clone();
        let has_pending_restore = restore_state.overlay_id() == Some(self.id);
        if !is_focused && !has_pending_restore {
            return;
        }

        if let OverlayFocusRestoreState::Blocked {
            overlay,
            blocked_by,
            resume: _,
        } = &restore_state
            && *overlay == self.id
            && self
                .core
                .0
                .borrow()
                .focused
                .as_ref()
                .is_some_and(|focused| Rc::ptr_eq(focused, blocked_by))
        {
            match options {
                Some(options) => {
                    self.core.0.borrow_mut().overlay_focus_restore =
                        OverlayFocusRestoreState::Blocked {
                            overlay: self.id,
                            blocked_by: blocked_by.clone(),
                            resume: OverlayBlockedFocusResume::FocusTarget(options.target),
                        };
                }
                None => self.core.clear_overlay_focus_restore(),
            }
            self.core.request_render();
            return;
        }

        self.core.clear_overlay_focus_restore_for(self.id);
        if is_focused || options.is_some() {
            let top_visible = self.core.get_topmost_visible_overlay();
            let fallback_target = match top_visible {
                Some(top) if !Rc::ptr_eq(&top, &self.component) => Some(top),
                _ => self
                    .core
                    .0
                    .borrow()
                    .overlay_stack
                    .iter()
                    .find(|entry| entry.id == self.id)
                    .and_then(|entry| entry.pre_focus.clone()),
            };
            self.core.set_focus(match options {
                Some(options) => options.target,
                None => fallback_target,
            });
        }
        self.core.request_render();
    }

    /// Whether this overlay currently has focus.
    pub fn is_focused(&self) -> bool {
        self.core
            .0
            .borrow()
            .focused
            .as_ref()
            .is_some_and(|focused| Rc::ptr_eq(focused, &self.component))
    }
}

impl TuiCore {
    /// Render all mounted root components (`Container.render`).
    pub fn render_children(&self, width: usize) -> Vec<Line> {
        let children = self.0.borrow().children.clone();
        let mut lines = Vec::new();
        for child in children {
            lines.extend(child.borrow_mut().render(width));
        }
        lines
    }
}
