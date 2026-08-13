//! Vertical scroll viewport.
//!
//! 1:1 port of `packages/tui/src/components/scroll-view.ts` (216 LOC).
//! The scrollbar auto-hide timer is exposed as a deadline the caller drives,
//! like the other timers of this crate (deviation class 1).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::layout_node::{LayoutNode, ScrollLayoutState};
use crate::tui::{Component, ComponentRef, Container};

/// Scrollbar visibility policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScrollViewScrollbar {
    /// Never show the scrollbar.
    #[default]
    Hidden,
    /// Show it while scrolling, then hide it again.
    Auto,
    /// Always show it (reserves a column).
    Always,
}

/// How wheel events behave at the scroll edges.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Overscroll {
    /// Pass the remaining delta to the parent scroll view.
    #[default]
    Chain,
    /// Swallow the remaining delta.
    Contain,
}

/// Options of a [`ScrollView`].
pub struct ScrollViewOptions {
    /// Follow the content end while it grows.
    pub follow_end: bool,
    /// Whether this is the primary scroll view.
    pub primary: bool,
    /// Overscroll behaviour.
    pub overscroll: Overscroll,
    /// Scrollbar policy.
    pub scrollbar: ScrollViewScrollbar,
    /// Style applied to the scrollbar thumb.
    pub scrollbar_style: Rc<dyn Fn(&str) -> String>,
    /// Hide delay of the transient scrollbar.
    pub scrollbar_hide_delay_ms: u64,
}

impl Default for ScrollViewOptions {
    fn default() -> Self {
        Self {
            follow_end: false,
            primary: false,
            overscroll: Overscroll::Chain,
            scrollbar: ScrollViewScrollbar::Hidden,
            scrollbar_style: Rc::new(|text| format!("\x1b[100m{text}\x1b[49m")),
            scrollbar_hide_delay_ms: 1000,
        }
    }
}

/// Options of [`ScrollView::scroll_to`].
#[derive(Debug, Clone, Copy, Default)]
pub struct ScrollToOptions {
    /// Keep follow-end disabled even when the target is the content end.
    pub disable_follow: bool,
}

/// Shared scroll state; the layout engine reads and updates it.
pub struct ScrollViewState {
    follow_end: bool,
    primary: bool,
    overscroll: Overscroll,
    scrollbar: ScrollViewScrollbar,
    scrollbar_style: Rc<dyn Fn(&str) -> String>,
    scrollbar_hide_delay_ms: u64,
    current_scroll_top: usize,
    content_height: usize,
    current_viewport_height: usize,
    following_end: bool,
    follow_suppressed_at_end: bool,
    transient_scrollbar_visible: bool,
    scrollbar_active: bool,
    scrollbar_hide_deadline: Option<Instant>,
    render_requested: bool,
}

impl ScrollViewState {
    /// Current scroll offset.
    pub fn scroll_top(&self) -> usize {
        self.current_scroll_top
    }

    /// Whether the view sticks to the content end.
    pub fn is_following_end(&self) -> bool {
        self.following_end
    }

    /// Height of the visible viewport.
    pub fn viewport_height(&self) -> usize {
        self.current_viewport_height
    }

    /// Current scrollbar policy.
    pub fn scrollbar(&self) -> ScrollViewScrollbar {
        self.scrollbar
    }

    /// Style applied to the scrollbar thumb.
    pub fn scrollbar_style(&self) -> Rc<dyn Fn(&str) -> String> {
        Rc::clone(&self.scrollbar_style)
    }

    /// Whether the scrollbar is currently drawn.
    pub fn is_scrollbar_visible(&self) -> bool {
        if self.scrollbar == ScrollViewScrollbar::Always {
            return self.current_viewport_height > 0;
        }
        self.scrollbar == ScrollViewScrollbar::Auto
            && self.content_height > self.current_viewport_height
            && self.transient_scrollbar_visible
    }

    /// Overscroll behaviour.
    pub fn overscroll(&self) -> Overscroll {
        self.overscroll
    }

    /// Whether a render was requested since the last check.
    pub fn take_render_request(&mut self) -> bool {
        std::mem::take(&mut self.render_requested)
    }

    /// When the transient scrollbar should disappear.
    pub fn scrollbar_hide_deadline(&self) -> Option<Instant> {
        self.scrollbar_hide_deadline
    }

    /// Hide the transient scrollbar once its deadline has passed.
    pub fn fire_scrollbar_hide(&mut self) {
        self.scrollbar_hide_deadline = None;
        self.transient_scrollbar_visible = false;
        self.render_requested = true;
    }

    /// Switch the scrollbar policy.
    pub fn set_scrollbar(&mut self, scrollbar: ScrollViewScrollbar) {
        if scrollbar == self.scrollbar {
            return;
        }
        self.scrollbar = scrollbar;
        if scrollbar != ScrollViewScrollbar::Auto {
            self.hide_transient_scrollbar();
        } else if self.scrollbar_active {
            self.mark_scrollbar_activity();
        }
        self.render_requested = true;
    }

    fn mark_scrollbar_activity(&mut self) {
        if self.scrollbar != ScrollViewScrollbar::Auto
            || self.content_height <= self.current_viewport_height
        {
            return;
        }
        self.transient_scrollbar_visible = true;
        self.scrollbar_hide_deadline = None;
        if self.scrollbar_active {
            return;
        }
        self.scrollbar_hide_deadline =
            Some(Instant::now() + Duration::from_millis(self.scrollbar_hide_delay_ms));
    }

    fn hide_transient_scrollbar(&mut self) {
        self.transient_scrollbar_visible = false;
        self.scrollbar_hide_deadline = None;
    }

    /// Mark the scrollbar as actively dragged.
    pub fn set_scrollbar_active(&mut self, active: bool) {
        if active == self.scrollbar_active {
            return;
        }
        self.scrollbar_active = active;
        self.mark_scrollbar_activity();
    }

    /// Scroll to an absolute offset.
    pub fn scroll_to(&mut self, scroll_top: i64, options: ScrollToOptions) {
        let max_scroll_top = self
            .content_height
            .saturating_sub(self.current_viewport_height) as i64;
        let next = scroll_top.clamp(0, max_scroll_top) as usize;
        let next_follow_suppressed_at_end = options.disable_follow && next as i64 == max_scroll_top;
        let next_following_end =
            !next_follow_suppressed_at_end && self.follow_end && next as i64 == max_scroll_top;
        if next == self.current_scroll_top
            && next_following_end == self.following_end
            && next_follow_suppressed_at_end == self.follow_suppressed_at_end
        {
            return;
        }
        let moved = next != self.current_scroll_top;
        self.current_scroll_top = next;
        self.following_end = next_following_end;
        self.follow_suppressed_at_end = next_follow_suppressed_at_end;
        if moved {
            self.mark_scrollbar_activity();
        }
        self.render_requested = true;
    }

    /// Scroll by a relative amount; returns the unconsumed delta.
    pub fn scroll_by(&mut self, lines: i64) -> i64 {
        if lines == 0 {
            return 0;
        }
        let max_scroll_top = self
            .content_height
            .saturating_sub(self.current_viewport_height) as i64;
        let start = if self.following_end {
            max_scroll_top
        } else {
            self.current_scroll_top as i64
        };
        let next = (start + lines).clamp(0, max_scroll_top);
        let moved = next - start;
        let was_following_end = self.following_end;
        self.current_scroll_top = next as usize;
        self.following_end = self.follow_end && next == max_scroll_top;
        self.follow_suppressed_at_end = false;
        if moved != 0 {
            self.mark_scrollbar_activity();
        }
        if moved != 0 || self.following_end != was_following_end {
            self.render_requested = true;
        }
        lines - moved
    }

    /// Scroll to the top.
    pub fn scroll_to_start(&mut self) {
        let next_following_end =
            self.follow_end && self.content_height <= self.current_viewport_height;
        let changed = self.current_scroll_top != 0 || self.following_end != next_following_end;
        self.current_scroll_top = 0;
        self.following_end = next_following_end;
        self.follow_suppressed_at_end = false;
        if changed {
            self.mark_scrollbar_activity();
            self.render_requested = true;
        }
    }

    /// Scroll to the bottom.
    pub fn scroll_to_end(&mut self) {
        let next = self
            .content_height
            .saturating_sub(self.current_viewport_height);
        let changed = self.current_scroll_top != next || self.following_end != self.follow_end;
        self.current_scroll_top = next;
        self.following_end = self.follow_end;
        self.follow_suppressed_at_end = false;
        if changed {
            self.mark_scrollbar_activity();
            self.render_requested = true;
        }
    }
}

impl ScrollLayoutState for ScrollViewState {
    fn scroll_top(&self) -> usize {
        self.current_scroll_top
    }

    fn primary(&self) -> bool {
        self.primary
    }

    fn overscroll_chain(&self) -> bool {
        self.overscroll == Overscroll::Chain
    }

    fn viewport_height(&self) -> usize {
        self.current_viewport_height
    }

    fn get_content_width(&self, width: usize) -> usize {
        if self.scrollbar == ScrollViewScrollbar::Always && width > 1 {
            width - 1
        } else {
            width
        }
    }

    fn scrollbar_visible(&self) -> bool {
        ScrollViewState::is_scrollbar_visible(self)
    }

    fn scrollbar_style_fn(&self) -> Rc<dyn Fn(&str) -> String> {
        Rc::clone(&self.scrollbar_style)
    }

    fn scroll_by_lines(&mut self, lines: i64) -> i64 {
        ScrollViewState::scroll_by(self, lines)
    }

    fn scroll_to_line(&mut self, scroll_top: i64, disable_follow: bool) {
        ScrollViewState::scroll_to(self, scroll_top, ScrollToOptions { disable_follow });
    }

    fn scroll_to_start_line(&mut self) {
        ScrollViewState::scroll_to_start(self);
    }

    fn scroll_to_end_line(&mut self) {
        ScrollViewState::scroll_to_end(self);
    }

    fn following_end(&self) -> bool {
        self.following_end
    }

    fn update_layout(&mut self, content_height: usize, viewport_height: usize) {
        self.content_height = content_height;
        self.current_viewport_height = viewport_height;
        let max_scroll_top = self
            .content_height
            .saturating_sub(self.current_viewport_height);
        if self.following_end {
            self.current_scroll_top = max_scroll_top;
        } else {
            self.current_scroll_top = self.current_scroll_top.min(max_scroll_top);
        }
        if self.current_scroll_top < max_scroll_top {
            self.follow_suppressed_at_end = false;
        }
        if self.follow_end
            && self.current_scroll_top == max_scroll_top
            && !self.follow_suppressed_at_end
        {
            self.following_end = true;
        }
        if self.content_height <= self.current_viewport_height {
            self.hide_transient_scrollbar();
        }
    }
}

/// Vertical scroll viewport around exactly one child.
pub struct ScrollView {
    container: Container,
    child: ComponentRef,
    state: Rc<RefCell<ScrollViewState>>,
}

impl ScrollView {
    /// New scroll view around `component`.
    pub fn new(component: ComponentRef, options: ScrollViewOptions) -> Self {
        let mut container = Container::new();
        container.add_child(component.clone());
        Self {
            container,
            child: component,
            state: Rc::new(RefCell::new(ScrollViewState {
                follow_end: options.follow_end,
                primary: options.primary,
                overscroll: options.overscroll,
                scrollbar: options.scrollbar,
                scrollbar_style: options.scrollbar_style,
                scrollbar_hide_delay_ms: options.scrollbar_hide_delay_ms,
                current_scroll_top: 0,
                content_height: 0,
                current_viewport_height: 0,
                following_end: options.follow_end,
                follow_suppressed_at_end: false,
                transient_scrollbar_visible: false,
                scrollbar_active: false,
                scrollbar_hide_deadline: None,
                render_requested: false,
            })),
        }
    }

    /// Shared scroll state.
    pub fn state(&self) -> Rc<RefCell<ScrollViewState>> {
        Rc::clone(&self.state)
    }
}

impl Component for ScrollView {
    fn render(&mut self, width: usize) -> Vec<String> {
        let content_width = {
            let state = self.state.borrow();
            ScrollLayoutState::get_content_width(&*state, width)
        };
        let lines = self.child.borrow_mut().render(content_width);
        if content_width == width {
            lines
        } else {
            lines.into_iter().map(|line| format!("{line} ")).collect()
        }
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }

    fn as_container(&self) -> Option<&Container> {
        Some(&self.container)
    }

    fn layout_node(&self) -> Option<LayoutNode> {
        Some(LayoutNode::Scroll {
            component: self.child.clone(),
            state: Rc::clone(&self.state) as Rc<RefCell<dyn ScrollLayoutState>>,
        })
    }
}

/// Whether the scrollbar of a layout scroll state is currently drawn.
///
/// The layout engine only sees `dyn ScrollLayoutState`; this helper downcasts
/// through the concrete state the crate always uses.
pub(crate) fn scrollbar_visible(state: &crate::layout_node::ScrollStateRef) -> bool {
    state.borrow().scrollbar_visible()
}

/// Style function of a layout scroll state.
pub(crate) fn scrollbar_style(
    state: &crate::layout_node::ScrollStateRef,
) -> Rc<dyn Fn(&str) -> String> {
    state.borrow().scrollbar_style_fn()
}
