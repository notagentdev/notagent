//! Box layout engine for the alternate screen.
//!
//! 1:1 port of `packages/tui/src/layout.ts` (410 LOC): layout tree, per-frame
//! render cache, clipping, the `paintBox` fast path, scrollbar geometry and hit
//! testing.

use std::collections::HashMap;
use std::rc::Rc;

use crate::components::stack::{allocate_stack_sizes, visible_stack_entries};
use crate::layout_node::{
    LayoutNode, LayoutViewport, ScrollStateRef, StackAlign, StackBasis, StackKind, get_layout_node,
};
use crate::terminal_image::{crop_kitty_image_line, get_kitty_image_metadata, is_image_line};
use crate::tui::{CURSOR_MARKER, ComponentRef, composite_tui_line};
use crate::utils::{extract_ansi_code, get_grapheme_cell_range, slice_by_column, visible_width};

/// Rectangle in cell coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LayoutRect {
    /// Left edge.
    pub x: i64,
    /// Top edge.
    pub y: i64,
    /// Width in columns.
    pub width: i64,
    /// Height in rows.
    pub height: i64,
}

/// One node of the laid out tree.
pub struct LayoutBox {
    /// The component this box belongs to.
    pub component: ComponentRef,
    /// Placement of the box.
    pub rect: LayoutRect,
    /// Visible part after clipping against the ancestors.
    pub clip: LayoutRect,
    /// Child boxes.
    pub children: Vec<LayoutBox>,
    /// Rendered lines of a leaf box.
    pub lines: Option<Vec<String>>,
    /// First rendered line shown (used to keep the cursor visible).
    pub line_offset: usize,
    /// Scroll state when this box is a scroll viewport.
    pub scroll_view: Option<ScrollStateRef>,
    /// Rendered content lines of a scroll viewport.
    pub scroll_content_lines: Option<Vec<String>>,
    /// Painting layer.
    pub layer: usize,
}

/// A laid out and painted frame.
pub struct LayoutFrame {
    /// Root of the layout tree.
    pub root: LayoutBox,
    /// Frame width.
    pub width: usize,
    /// Frame height.
    pub height: usize,
    /// Painted lines (exactly `height` entries).
    pub lines: Vec<String>,
    /// Primary scroll view of this frame, if any.
    pub primary_scroll_view: Option<ScrollStateRef>,
}

/// Geometry of a scroll view's scrollbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbarGeometry {
    /// Column the thumb is drawn in.
    pub column: i64,
    /// Top of the track.
    pub track_top: i64,
    /// Height of the track.
    pub track_height: i64,
    /// Top of the thumb.
    pub thumb_top: i64,
    /// Height of the thumb.
    pub thumb_height: i64,
    /// Largest possible scroll offset.
    pub max_scroll_top: i64,
}

struct LayoutContext {
    viewport: LayoutViewport,
    render_cache: HashMap<usize, HashMap<usize, Vec<String>>>,
    render_requested: bool,
    primary_scroll_view: Option<ScrollStateRef>,
}

fn component_key(component: &ComponentRef) -> usize {
    Rc::as_ptr(component).cast::<()>() as usize
}

fn intersect(a: LayoutRect, b: LayoutRect) -> LayoutRect {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = (a.x + a.width).min(b.x + b.width);
    let bottom = (a.y + a.height).min(b.y + b.height);
    LayoutRect {
        x,
        y,
        width: (right - x).max(0),
        height: (bottom - y).max(0),
    }
}

fn render_cached(context: &mut LayoutContext, component: &ComponentRef, width: i64) -> Vec<String> {
    let safe_width = width.max(1) as usize;
    let key = component_key(component);
    if let Some(lines) = context
        .render_cache
        .get(&key)
        .and_then(|widths| widths.get(&safe_width))
    {
        return lines.clone();
    }
    let lines = component.borrow_mut().render(safe_width);
    context
        .render_cache
        .entry(key)
        .or_default()
        .insert(safe_width, lines.clone());
    lines
}

fn measure_height(context: &mut LayoutContext, component: &ComponentRef, width: i64) -> usize {
    render_cached(context, component, width).len()
}

fn measure_width(context: &mut LayoutContext, component: &ComponentRef, width: i64) -> usize {
    render_cached(context, component, width)
        .iter()
        .map(|line| visible_width(line))
        .max()
        .unwrap_or(0)
}

fn translate_box(layout_box: &mut LayoutBox, delta_y: i64) {
    layout_box.rect.y += delta_y;
    for child in &mut layout_box.children {
        translate_box(child, delta_y);
    }
}

fn update_clips(layout_box: &mut LayoutBox, parent_clip: LayoutRect) {
    layout_box.clip = intersect(parent_clip, layout_box.rect);
    let clip = layout_box.clip;
    for child in &mut layout_box.children {
        update_clips(child, clip);
    }
}

fn layout_component(
    context: &mut LayoutContext,
    component: &ComponentRef,
    x: i64,
    y: i64,
    width: i64,
    height: Option<i64>,
    clip: LayoutRect,
) -> LayoutBox {
    let safe_width = width.max(1);
    let node = get_layout_node(component);
    let Some(node) = node else {
        let lines = render_cached(context, component, safe_width);
        let allocated_height = height.map_or(lines.len() as i64, |height| height.max(0));
        let mut line_offset = 0;
        if lines.len() as i64 > allocated_height
            && allocated_height > 0
            && let Some(cursor_line) = lines.iter().position(|line| line.contains(CURSOR_MARKER))
            && cursor_line as i64 >= allocated_height
        {
            line_offset = (cursor_line as i64 - allocated_height + 1) as usize;
        }
        let rect = LayoutRect {
            x,
            y,
            width: safe_width,
            height: allocated_height,
        };
        return LayoutBox {
            component: component.clone(),
            rect,
            clip: intersect(clip, rect),
            children: Vec::new(),
            lines: Some(lines),
            line_offset,
            scroll_view: None,
            scroll_content_lines: None,
            layer: 0,
        };
    };

    match node {
        LayoutNode::Scroll {
            component: child,
            state,
        } => {
            let previous_scroll_top = state.borrow().scroll_top() as i64;
            let content_width = state.borrow().get_content_width(safe_width as usize) as i64;
            let mut child_box = layout_component(
                context,
                &child,
                x,
                y - previous_scroll_top,
                content_width,
                None,
                clip,
            );
            let content_height = child_box.rect.height;
            let viewport_height = height.map_or(content_height, |height| height.max(0));
            state.borrow_mut().update_layout(
                content_height.max(0) as usize,
                viewport_height.max(0) as usize,
            );
            let new_scroll_top = state.borrow().scroll_top() as i64;
            translate_box(&mut child_box, previous_scroll_top - new_scroll_top);
            if state.borrow().primary() || context.primary_scroll_view.is_none() {
                context.primary_scroll_view = Some(Rc::clone(&state));
            }
            let rect = LayoutRect {
                x,
                y,
                width: safe_width,
                height: viewport_height,
            };
            let child_clip = intersect(clip, rect);
            update_clips(&mut child_box, child_clip);
            let scroll_content_lines = render_cached(context, &child, content_width);
            LayoutBox {
                component: component.clone(),
                rect,
                clip: child_clip,
                children: vec![child_box],
                lines: None,
                line_offset: 0,
                scroll_view: Some(state),
                scroll_content_lines: Some(scroll_content_lines),
                layer: 0,
            }
        }
        LayoutNode::Stack {
            kind,
            entries,
            gap,
            align,
        } => {
            let entries = visible_stack_entries(&entries, context.viewport);
            let gap_total = (entries.len().saturating_sub(1) * gap) as i64;
            if kind == StackKind::VStack {
                let intrinsic_heights: Vec<usize> = entries
                    .iter()
                    .map(|entry| match entry.basis {
                        Some(StackBasis::Size(size)) => size,
                        _ => measure_height(context, &entry.component, safe_width),
                    })
                    .collect();
                let sizes = allocate_stack_sizes(
                    &entries,
                    &intrinsic_heights,
                    height.map(|height| height.max(0) as usize),
                    gap,
                );
                let natural_height = sizes.iter().sum::<usize>() as i64 + gap_total;
                let allocated_height = height.map_or(natural_height, |height| height.max(0));
                let rect = LayoutRect {
                    x,
                    y,
                    width: safe_width,
                    height: allocated_height,
                };
                let box_clip = intersect(clip, rect);
                let mut children = Vec::new();
                let mut child_y = y;
                for (index, entry) in entries.iter().enumerate() {
                    children.push(layout_component(
                        context,
                        &entry.component,
                        x,
                        child_y,
                        safe_width,
                        Some(sizes[index] as i64),
                        box_clip,
                    ));
                    child_y += sizes[index] as i64 + gap as i64;
                }
                return LayoutBox {
                    component: component.clone(),
                    rect,
                    clip: box_clip,
                    children,
                    lines: None,
                    line_offset: 0,
                    scroll_view: None,
                    scroll_content_lines: None,
                    layer: 0,
                };
            }

            let intrinsic_widths: Vec<usize> = entries
                .iter()
                .map(|entry| match entry.basis {
                    Some(StackBasis::Size(size)) => size,
                    _ => measure_width(context, &entry.component, safe_width),
                })
                .collect();
            let widths =
                allocate_stack_sizes(&entries, &intrinsic_widths, Some(safe_width as usize), gap);
            let intrinsic_heights: Vec<usize> = entries
                .iter()
                .enumerate()
                .map(|(index, entry)| {
                    measure_height(context, &entry.component, (widths[index] as i64).max(1))
                })
                .collect();
            let allocated_height = height.map_or_else(
                || intrinsic_heights.iter().copied().max().unwrap_or(0) as i64,
                |height| height.max(0),
            );
            let rect = LayoutRect {
                x,
                y,
                width: safe_width,
                height: allocated_height,
            };
            let box_clip = intersect(clip, rect);
            let mut children = Vec::new();
            let mut child_x = x;
            for (index, entry) in entries.iter().enumerate() {
                let natural_child_height = intrinsic_heights[index] as i64;
                let child_height = if align == StackAlign::Stretch {
                    allocated_height
                } else {
                    allocated_height.min(natural_child_height)
                };
                let mut child_y = y;
                match align {
                    StackAlign::Center => child_y += (allocated_height - child_height) / 2,
                    StackAlign::End => child_y += allocated_height - child_height,
                    _ => {}
                }
                let child_width = widths[index] as i64;
                if child_width == 0 {
                    children.push(LayoutBox {
                        component: entry.component.clone(),
                        rect: LayoutRect {
                            x: child_x,
                            y: child_y,
                            width: 0,
                            height: child_height,
                        },
                        clip: LayoutRect {
                            x: child_x,
                            y: child_y,
                            width: 0,
                            height: 0,
                        },
                        children: Vec::new(),
                        lines: None,
                        line_offset: 0,
                        scroll_view: None,
                        scroll_content_lines: None,
                        layer: 0,
                    });
                } else {
                    children.push(layout_component(
                        context,
                        &entry.component,
                        child_x,
                        child_y,
                        child_width,
                        Some(child_height),
                        box_clip,
                    ));
                }
                child_x += child_width + gap as i64;
            }
            LayoutBox {
                component: component.clone(),
                rect,
                clip: box_clip,
                children,
                lines: None,
                line_offset: 0,
                scroll_view: None,
                scroll_content_lines: None,
                layer: 0,
            }
        }
    }
}

/// Strip leading OSC 133 zone markers (`^(?:\x1b]133;[ABC](?:\x07|\x1b\\))+`).
fn strip_osc133_zone_prefix(line: &str) -> &str {
    let mut rest = line;
    loop {
        let Some(after) = rest.strip_prefix("\x1b]133;") else {
            return rest;
        };
        let Some(kind) = after.chars().next() else {
            return rest;
        };
        if !matches!(kind, 'A' | 'B' | 'C') {
            return rest;
        }
        let after_kind = &after[kind.len_utf8()..];
        if let Some(next) = after_kind.strip_prefix('\x07') {
            rest = next;
        } else if let Some(next) = after_kind.strip_prefix("\x1b\\") {
            rest = next;
        } else {
            return rest;
        }
    }
}

fn style_scrollbar_cell(
    line: &str,
    column: i64,
    total_width: i64,
    style: &dyn Fn(&str) -> String,
) -> String {
    if is_image_line(line) {
        return line.to_string();
    }
    let column = column.max(0) as usize;
    let grapheme_range = get_grapheme_cell_range(line, column);
    let start = grapheme_range.map_or(column, |range| range.start);
    let end = grapheme_range.map_or(column + 1, |range| range.end);
    let before = slice_by_column(line, 0, start, true);
    let target = slice_by_column(line, start, end - start, true);
    let after = slice_by_column(
        line,
        end,
        (total_width.max(0) as usize).saturating_sub(end),
        true,
    );

    let mut target_prefix = String::new();
    let mut target_index = 0;
    while target_index < target.len() {
        let Some(ansi) = extract_ansi_code(&target, target_index) else {
            break;
        };
        target_prefix.push_str(ansi.code);
        target_index += ansi.length;
    }
    let target_text = if target[target_index..].is_empty() {
        " ".repeat(end - start)
    } else {
        target[target_index..].to_string()
    };
    let before_padding = " ".repeat(start.saturating_sub(visible_width(&before)));
    format!(
        "{before}{before_padding}{target_prefix}{}{after}",
        style(&target_text)
    )
}

/// Geometry of the scrollbar of a scroll view box.
pub fn get_scrollbar_geometry(layout_box: &LayoutBox) -> Option<ScrollbarGeometry> {
    let scroll_view = layout_box.scroll_view.as_ref()?;
    if !scroll_view_is_scrollbar_visible(scroll_view)
        || layout_box.rect.width <= 0
        || layout_box.rect.height <= 0
    {
        return None;
    }

    let content_height = layout_box
        .children
        .first()
        .map(|child| child.rect.height)
        .or_else(|| {
            layout_box
                .scroll_content_lines
                .as_ref()
                .map(|lines| lines.len() as i64)
        })
        .unwrap_or(0);
    let track_height = layout_box.rect.height;
    let min_thumb_height = 2.min(track_height);
    let thumb_height = if content_height <= 0 {
        track_height
    } else {
        min_thumb_height.max(track_height.min(
            ((track_height as f64 * track_height as f64) / content_height as f64).round() as i64,
        ))
    };
    let max_scroll_top = (content_height - track_height).max(0);
    let max_thumb_top = track_height - thumb_height;
    let thumb_offset = if max_scroll_top == 0 {
        0
    } else {
        ((scroll_view.borrow().scroll_top() as f64 / max_scroll_top as f64) * max_thumb_top as f64)
            .round() as i64
    };
    let column = layout_box.rect.x + layout_box.rect.width - 1;
    if column < layout_box.clip.x || column >= layout_box.clip.x + layout_box.clip.width {
        return None;
    }

    Some(ScrollbarGeometry {
        column,
        track_top: layout_box.rect.y,
        track_height,
        thumb_top: layout_box.rect.y + thumb_offset,
        thumb_height,
        max_scroll_top,
    })
}

fn scroll_view_is_scrollbar_visible(state: &ScrollStateRef) -> bool {
    // `ScrollLayoutState` deliberately stays minimal; the concrete state knows
    // its scrollbar policy.
    crate::components::scroll_view::scrollbar_visible(state)
}

fn paint_scrollbar(layout_box: &LayoutBox, screen: &mut [String], total_width: i64) {
    let Some(geometry) = get_scrollbar_geometry(layout_box) else {
        return;
    };
    let Some(scroll_view) = layout_box.scroll_view.as_ref() else {
        return;
    };
    let style = crate::components::scroll_view::scrollbar_style(scroll_view);
    for offset in 0..geometry.thumb_height {
        let row = geometry.thumb_top + offset;
        if row < layout_box.clip.y
            || row >= layout_box.clip.y + layout_box.clip.height
            || row < 0
            || row >= screen.len() as i64
        {
            continue;
        }
        let index = row as usize;
        screen[index] = style_scrollbar_cell(&screen[index], geometry.column, total_width, &*style);
    }
}

fn paint_box(layout_box: &LayoutBox, screen: &mut Vec<String>, total_width: i64) {
    if let Some(lines) = &layout_box.lines {
        let offset = layout_box.line_offset as i64;
        let first_row = layout_box.rect.y.max(layout_box.clip.y).max(0);
        let last_row = (layout_box.rect.y + layout_box.rect.height)
            .min(layout_box.clip.y + layout_box.clip.height)
            .min(screen.len() as i64);
        for row in first_row..last_row {
            let source_index = offset + row - layout_box.rect.y;
            if source_index < 0 {
                continue;
            }
            let Some(source_line) = lines.get(source_index as usize) else {
                continue;
            };
            let mut line = strip_osc133_zone_prefix(source_line).to_string();
            if let Some(metadata) = get_kitty_image_metadata(&line) {
                let clip_bottom =
                    (screen.len() as i64).min(layout_box.clip.y + layout_box.clip.height);
                let visible_rows = (metadata.rows as i64).min(clip_bottom - row).max(0) as usize;
                if visible_rows < metadata.rows {
                    line = crop_kitty_image_line(&line, 0, visible_rows);
                }
            }
            let index = row as usize;
            // Fast path: a full-width box painting onto an untouched row can use
            // the source line directly. Compositing would rebuild the row through
            // ANSI/grapheme segmentation every frame; padding is unnecessary
            // because rows are written with erase-line and the final width clamp
            // still truncates over-wide lines.
            if layout_box.rect.x == 0
                && layout_box.rect.width >= total_width
                && (is_image_line(&line) || screen[index].is_empty())
            {
                screen[index] = line;
            } else {
                screen[index] = composite_tui_line(
                    &screen[index],
                    &line,
                    layout_box.rect.x.max(0) as usize,
                    layout_box.rect.width.max(0) as usize,
                    total_width.max(0) as usize,
                );
            }
        }
    }
    for child in &layout_box.children {
        paint_box(child, screen, total_width);
    }

    if let (Some(scroll_view), Some(content_lines)) = (
        layout_box.scroll_view.as_ref(),
        layout_box.scroll_content_lines.as_ref(),
    ) {
        let scroll_top = scroll_view.borrow().scroll_top();
        if scroll_top > 0 && layout_box.rect.height > 0 {
            for image_row in (0..scroll_top).rev() {
                let image_line = content_lines.get(image_row).cloned().unwrap_or_default();
                if let Some(metadata) = get_kitty_image_metadata(&image_line) {
                    let hidden_rows = scroll_top - image_row;
                    if hidden_rows < metadata.rows {
                        let visible_rows =
                            (layout_box.rect.height as usize).min(metadata.rows - hidden_rows);
                        let cropped = crop_kitty_image_line(&image_line, hidden_rows, visible_rows);
                        if layout_box.rect.x == 0 && layout_box.rect.width >= total_width {
                            let index = layout_box.rect.y.max(0) as usize;
                            if index < screen.len() {
                                screen[index] = cropped;
                            }
                        }
                    }
                    break;
                }
                if !image_line.is_empty() {
                    break;
                }
            }
        }
    }

    paint_scrollbar(layout_box, screen, total_width);
}

/// Lay out and paint one frame.
pub fn render_layout_frame(root: &ComponentRef, width: usize, height: usize) -> LayoutFrame {
    let safe_width = width.max(1);
    let safe_height = height.max(1);
    let mut context = LayoutContext {
        viewport: LayoutViewport {
            width: safe_width,
            height: safe_height,
        },
        render_cache: HashMap::new(),
        render_requested: false,
        primary_scroll_view: None,
    };
    let root_box = layout_component(
        &mut context,
        root,
        0,
        0,
        safe_width as i64,
        Some(safe_height as i64),
        LayoutRect {
            x: 0,
            y: 0,
            width: safe_width as i64,
            height: safe_height as i64,
        },
    );
    let mut lines = vec![String::new(); safe_height];
    paint_box(&root_box, &mut lines, safe_width as i64);
    let _ = context.render_requested;
    LayoutFrame {
        root: root_box,
        width: safe_width,
        height: safe_height,
        lines,
        primary_scroll_view: context.primary_scroll_view,
    }
}

fn contains_point(rect: LayoutRect, x: i64, y: i64) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

/// Find the box of a given scroll view.
pub fn get_scroll_view_box<'a>(
    frame: &'a LayoutFrame,
    scroll_view: &ScrollStateRef,
) -> Option<&'a LayoutBox> {
    fn visit<'a>(layout_box: &'a LayoutBox, scroll_view: &ScrollStateRef) -> Option<&'a LayoutBox> {
        if layout_box
            .scroll_view
            .as_ref()
            .is_some_and(|candidate| Rc::ptr_eq(candidate, scroll_view))
        {
            return Some(layout_box);
        }
        layout_box
            .children
            .iter()
            .find_map(|child| visit(child, scroll_view))
    }
    visit(&frame.root, scroll_view)
}

/// Scroll views under a point, deepest first.
pub fn get_scroll_views_at(frame: &LayoutFrame, x: i64, y: i64) -> Vec<ScrollStateRef> {
    fn visit(
        layout_box: &LayoutBox,
        depth: usize,
        x: i64,
        y: i64,
        result: &mut Vec<(ScrollStateRef, usize)>,
    ) {
        if !contains_point(layout_box.clip, x, y) {
            return;
        }
        if let Some(scroll_view) = layout_box.scroll_view.as_ref()
            && contains_point(layout_box.rect, x, y)
        {
            result.push((Rc::clone(scroll_view), depth));
        }
        for child in &layout_box.children {
            visit(child, depth + 1, x, y, result);
        }
    }
    let mut result: Vec<(ScrollStateRef, usize)> = Vec::new();
    visit(&frame.root, 0, x, y, &mut result);
    result.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    result
        .into_iter()
        .map(|(scroll_view, _)| scroll_view)
        .collect()
}
