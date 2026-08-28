use crate::components::stack::{
    Stack, StackEntryOptions, StackOptions, allocate_stack_sizes, visible_stack_entries,
};
use crate::layout_node::{LayoutNode, LayoutViewport, StackAlign, StackKind};
use crate::tui::{Component, ComponentRef, Container, Line, composite_tui_line, shared_lines};
use crate::utils::visible_width;

/// Horizontal stack; composes children side by side.
pub struct HStack {
    stack: Stack,
}

impl HStack {
    /// New horizontal stack.
    pub fn new(options: StackOptions) -> Self {
        Self {
            stack: Stack::new(StackKind::HStack, options),
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

impl Component for HStack {
    fn render(&mut self, width: usize) -> Vec<Line> {
        let safe_width = width.max(1);
        let viewport = LayoutViewport {
            width: safe_width,
            height: usize::MAX,
        };
        let Some(LayoutNode::Stack {
            entries,
            gap,
            align,
            ..
        }) = self.stack.layout_node()
        else {
            return Vec::new();
        };
        let entries = visible_stack_entries(&entries, viewport);
        if entries.is_empty() {
            return Vec::new();
        }

        let intrinsic_widths: Vec<usize> = entries
            .iter()
            .map(|entry| {
                entry
                    .component
                    .borrow_mut()
                    .render(safe_width)
                    .iter()
                    .map(|line| visible_width(line))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let widths = allocate_stack_sizes(&entries, &intrinsic_widths, Some(safe_width), gap);
        let rendered: Vec<Vec<Line>> = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                if widths[index] == 0 {
                    Vec::new()
                } else {
                    entry.component.borrow_mut().render(widths[index])
                }
            })
            .collect();
        let height = rendered.iter().map(Vec::len).max().unwrap_or(0);
        let mut result = vec![String::new(); height];
        let mut x = 0;
        for (index, lines) in rendered.iter().enumerate() {
            let child_width = widths[index];
            let offset: i64 = match align {
                StackAlign::Center => (height as i64 - lines.len() as i64) / 2,
                StackAlign::End => height as i64 - lines.len() as i64,
                _ => 0,
            };
            for (row, line) in lines.iter().enumerate() {
                let target = row as i64 + offset;
                if target < 0 || target >= result.len() as i64 {
                    continue;
                }
                let target = target as usize;
                result[target] =
                    composite_tui_line(&result[target], line, x, child_width, safe_width);
            }
            x += child_width + gap;
        }
        shared_lines(result)
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
