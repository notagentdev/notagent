use std::cell::RefCell;
use std::rc::Rc;

use crate::tui::ComponentRef;

/// Viewport passed to a stack entry's visibility callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutViewport {
    /// Viewport width in columns.
    pub width: usize,
    /// Viewport height in rows.
    pub height: usize,
}

/// Basis of a stack entry: an explicit size or the intrinsic one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackBasis {
    /// Fixed size in cells.
    Size(usize),
    Auto,
}

/// One entry of a stack layout.
#[derive(Clone)]
pub struct StackLayoutEntry {
    /// The child component.
    pub component: ComponentRef,
    /// Base size before growing or shrinking.
    pub basis: Option<StackBasis>,
    /// Grow weight.
    pub grow: Option<usize>,
    /// Shrink weight.
    pub shrink: Option<usize>,
    /// Lower bound.
    pub min_size: Option<usize>,
    /// Upper bound.
    pub max_size: Option<usize>,
    /// Only lay out the entry while this returns `true`.
    pub visible: Option<Rc<dyn Fn(LayoutViewport) -> bool>>,
}

/// Cross-axis alignment of an hstack.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StackAlign {
    /// Stretch children to the stack height.
    #[default]
    Stretch,
    /// Align at the top.
    Start,
    /// Center vertically.
    Center,
    /// Align at the bottom.
    End,
}

/// Orientation of a stack layout node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackKind {
    /// Vertical stack.
    VStack,
    /// Horizontal stack.
    HStack,
}

/// Scroll state the layout engine reads and updates.
pub trait ScrollLayoutState {
    /// Current scroll offset in lines.
    fn scroll_top(&self) -> usize;
    /// Whether this scroll view is the primary one.
    fn primary(&self) -> bool;
    /// Whether wheel events chain to the parent at the edges.
    fn overscroll_chain(&self) -> bool;
    /// Height of the visible viewport.
    fn viewport_height(&self) -> usize;
    /// Content width for the given box width (reserves the scrollbar column).
    fn get_content_width(&self, width: usize) -> usize;
    /// Store the measured sizes and clamp the scroll offset.
    fn update_layout(&mut self, content_height: usize, viewport_height: usize);
    /// Whether the scrollbar is currently drawn.
    fn scrollbar_visible(&self) -> bool;
    /// Style applied to the scrollbar thumb.
    fn scrollbar_style_fn(&self) -> Rc<dyn Fn(&str) -> String>;
    /// Scroll by a relative amount; returns the unconsumed delta.
    fn scroll_by_lines(&mut self, lines: i64) -> i64;
    /// Scroll to an absolute offset.
    fn scroll_to_line(&mut self, scroll_top: i64, disable_follow: bool);
    /// Scroll to the top.
    fn scroll_to_start_line(&mut self);
    /// Scroll to the bottom.
    fn scroll_to_end_line(&mut self);
    /// Whether the view sticks to the content end.
    fn following_end(&self) -> bool;
    /// Mark the scrollbar as actively hovered or dragged.
    fn set_scrollbar_active_state(&mut self, active: bool);
}

pub type ScrollStateRef = Rc<RefCell<dyn ScrollLayoutState>>;

/// What a layout-aware component contributes to the layout tree.
#[derive(Clone)]
pub enum LayoutNode {
    /// A vertical or horizontal stack.
    Stack {
        /// Orientation.
        kind: StackKind,
        /// Child entries.
        entries: Vec<StackLayoutEntry>,
        /// Gap between children.
        gap: usize,
        /// Cross-axis alignment.
        align: StackAlign,
    },
    /// A scrollable viewport around a single child.
    Scroll {
        /// The scrolled child.
        component: ComponentRef,
        /// Shared scroll state.
        state: ScrollStateRef,
    },
}

/// Layout node of a component, if it is layout aware.
pub fn get_layout_node(component: &ComponentRef) -> Option<LayoutNode> {
    component.borrow().layout_node()
}
