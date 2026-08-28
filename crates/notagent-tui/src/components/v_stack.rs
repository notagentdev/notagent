use crate::components::stack::{
    Stack, StackEntryOptions, StackOptions, allocate_stack_sizes, visible_stack_entries,
};
use crate::layout_node::{LayoutNode, LayoutViewport, StackKind};
use crate::tui::{Component, ComponentRef, Container, Line};

/// Vertical stack; renders children top to bottom.
pub struct VStack {
    stack: Stack,
}

impl VStack {
    /// New vertical stack.
    pub fn new(options: StackOptions) -> Self {
        Self {
            stack: Stack::new(StackKind::VStack, options),
        }
    }

    /// Append a child with layout options.
    pub fn add_child_with(&mut self, component: ComponentRef, options: StackEntryOptions) {
        self.stack.add_child_with(component, options);
    }

    /// Append a child.
    pub fn add_child(&mut self, component: ComponentRef) {
        self.stack.add_child(component);
    }

    /// Remove a child.
    pub fn remove_child(&mut self, component: &ComponentRef) {
        self.stack.remove_child(component);
    }

    /// Drop all children.
    pub fn clear(&mut self) {
        self.stack.clear();
    }
}

impl Component for VStack {
    fn render(&mut self, width: usize) -> Vec<Line> {
        let viewport = LayoutViewport {
            width: width.max(1),
            height: usize::MAX,
        };
        let Some(LayoutNode::Stack { entries, gap, .. }) = self.stack.layout_node() else {
            return Vec::new();
        };
        let entries = visible_stack_entries(&entries, viewport);
        let rendered: Vec<Vec<Line>> = entries
            .iter()
            .map(|entry| entry.component.borrow_mut().render(viewport.width))
            .collect();
        let sizes = allocate_stack_sizes(
            &entries,
            &rendered.iter().map(Vec::len).collect::<Vec<_>>(),
            None,
            gap,
        );
        let mut lines: Vec<Line> = Vec::new();
        for (index, child_lines) in rendered.iter().enumerate() {
            if index > 0 {
                for _ in 0..gap {
                    lines.push(Line::from(""));
                }
            }
            let taken = child_lines.len().min(sizes[index]);
            lines.extend(child_lines[..taken].iter().cloned());
            for _ in taken..sizes[index] {
                lines.push(Line::from(""));
            }
        }
        lines
    }

    fn invalidate(&mut self) {
        self.stack.invalidate();
    }

    fn as_container(&self) -> Option<&Container> {
        self.stack.as_container()
    }

    fn layout_node(&self) -> Option<LayoutNode> {
        self.stack.layout_node()
    }
}
