//! Stack base class and the flexbox-like size allocation.
//!
//! 1:1 port of `packages/tui/src/components/stack.ts` (154 LOC).

use std::rc::Rc;

use crate::layout_node::{
    LayoutNode, LayoutViewport, StackAlign, StackBasis, StackKind, StackLayoutEntry,
};
use crate::tui::{Component, ComponentRef, Container, Line};

/// Per-entry options of a stack child.
#[derive(Clone, Default)]
pub struct StackEntryOptions {
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

/// Options of a stack.
#[derive(Debug, Clone, Copy, Default)]
pub struct StackOptions {
    /// Gap between children.
    pub gap: Option<usize>,
    /// Cross-axis alignment.
    pub align: Option<StackAlign>,
}

/// Shared implementation of `VStack` and `HStack`.
pub struct Stack {
    container: Container,
    entries: Vec<StackLayoutEntry>,
    gap: usize,
    align: StackAlign,
    layout_type: StackKind,
}

impl Stack {
    /// New stack of the given orientation.
    pub fn new(layout_type: StackKind, options: StackOptions) -> Self {
        Self {
            container: Container::new(),
            entries: Vec::new(),
            gap: options.gap.unwrap_or(0),
            align: options.align.unwrap_or_default(),
            layout_type,
        }
    }

    /// Append a child with layout options.
    pub fn add_child_with(&mut self, component: ComponentRef, options: StackEntryOptions) {
        self.container.add_child(component.clone());
        self.entries.push(StackLayoutEntry {
            component,
            basis: options.basis,
            grow: options.grow,
            shrink: options.shrink,
            min_size: options.min_size,
            max_size: options.max_size,
            visible: options.visible,
        });
    }

    /// Append a child with default layout options.
    pub fn add_child(&mut self, component: ComponentRef) {
        self.add_child_with(component, StackEntryOptions::default());
    }

    /// Remove a child.
    pub fn remove_child(&mut self, component: &ComponentRef) {
        self.container.remove_child(component);
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| Rc::ptr_eq(&entry.component, component))
        {
            self.entries.remove(index);
        }
    }

    /// Drop all children.
    pub fn clear(&mut self) {
        self.container.clear();
        self.entries.clear();
    }

    /// The children as a container (for rendering without the layout engine).
    pub fn container(&self) -> &Container {
        &self.container
    }

    /// Mutable access to the children container.
    pub fn container_mut(&mut self) -> &mut Container {
        &mut self.container
    }
}

impl Component for Stack {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.container.render(width)
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }

    fn as_container(&self) -> Option<&Container> {
        Some(&self.container)
    }

    fn layout_node(&self) -> Option<LayoutNode> {
        Some(LayoutNode::Stack {
            kind: self.layout_type,
            entries: self.entries.clone(),
            gap: self.gap,
            align: self.align,
        })
    }
}

/// Entries whose visibility callback accepts the current viewport.
pub fn visible_stack_entries(
    entries: &[StackLayoutEntry],
    viewport: LayoutViewport,
) -> Vec<StackLayoutEntry> {
    entries
        .iter()
        .filter(|entry| {
            entry
                .visible
                .as_ref()
                .is_none_or(|visible| visible(viewport))
        })
        .cloned()
        .collect()
}

fn clamp_size(size: i64, entry: &StackLayoutEntry) -> i64 {
    let min = entry.min_size.unwrap_or(0) as i64;
    let max = entry.max_size.map_or(i64::MAX, |max| max as i64).max(min);
    size.max(0).clamp(min, max)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DistributeMode {
    Grow,
    Shrink,
}

fn distribute(sizes: &mut [i64], entries: &[StackLayoutEntry], amount: i64, mode: DistributeMode) {
    let mut remaining = amount;
    while remaining > 0 {
        let candidates: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|(index, entry)| match mode {
                DistributeMode::Grow => {
                    entry.grow.unwrap_or(0) > 0
                        && sizes[*index] < entry.max_size.map_or(i64::MAX, |max| max as i64)
                }
                DistributeMode::Shrink => {
                    entry.shrink.unwrap_or(1) > 0
                        && sizes[*index] > entry.min_size.unwrap_or(0) as i64
                }
            })
            .map(|(index, _)| index)
            .collect();
        if candidates.is_empty() {
            return;
        }

        fn weight_of(
            entries: &[StackLayoutEntry],
            sizes: &[i64],
            mode: DistributeMode,
            index: usize,
        ) -> i64 {
            match mode {
                DistributeMode::Grow => entries[index].grow.unwrap_or(0) as i64,
                DistributeMode::Shrink => {
                    entries[index].shrink.unwrap_or(1) as i64 * sizes[index].max(1)
                }
            }
        }
        let total_weight: i64 = candidates
            .iter()
            .map(|index| weight_of(entries, sizes, mode, *index))
            .sum();
        let mut distributed = 0;
        for index in candidates {
            if remaining <= 0 {
                break;
            }
            let weight = weight_of(entries, sizes, mode, index);
            let proposed = (remaining * weight / total_weight.max(1)).max(1);
            let capacity = match mode {
                DistributeMode::Grow => {
                    entries[index].max_size.map_or(i64::MAX, |max| max as i64) - sizes[index]
                }
                DistributeMode::Shrink => {
                    sizes[index] - entries[index].min_size.unwrap_or(0) as i64
                }
            };
            let delta = remaining.min(proposed).min(capacity);
            if delta <= 0 {
                continue;
            }
            sizes[index] += if mode == DistributeMode::Grow {
                delta
            } else {
                -delta
            };
            remaining -= delta;
            distributed += delta;
        }
        if distributed == 0 {
            return;
        }
    }
}

/// Flexbox-like size allocation across stack entries.
pub fn allocate_stack_sizes(
    entries: &[StackLayoutEntry],
    intrinsic_sizes: &[usize],
    available_size: Option<usize>,
    gap: usize,
) -> Vec<usize> {
    let mut sizes: Vec<i64> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let base = match entry.basis {
                Some(StackBasis::Size(size)) => size as i64,
                Some(StackBasis::Auto) | None => {
                    intrinsic_sizes.get(index).copied().unwrap_or(0) as i64
                }
            };
            clamp_size(base, entry)
        })
        .collect();

    let Some(available_size) = available_size else {
        return sizes.into_iter().map(|size| size.max(0) as usize).collect();
    };

    let content_size =
        (available_size as i64 - (entries.len().saturating_sub(1) * gap) as i64).max(0);
    let total: i64 = sizes.iter().sum();
    match total.cmp(&content_size) {
        std::cmp::Ordering::Less => {
            distribute(
                &mut sizes,
                entries,
                content_size - total,
                DistributeMode::Grow,
            );
        }
        std::cmp::Ordering::Greater => {
            distribute(
                &mut sizes,
                entries,
                total - content_size,
                DistributeMode::Shrink,
            );
        }
        std::cmp::Ordering::Equal => {}
    }
    sizes.into_iter().map(|size| size.max(0) as usize).collect()
}
