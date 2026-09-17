use std::collections::BTreeSet;
use std::io::Write;

use crate::terminal_image::{delete_kitty_image, is_image_line};
use crate::tui::{Line, TuiCore, TuiMode, TuiStopOptions};
use crate::utils::visible_width;

const KITTY_SEQUENCE_PREFIX: &str = "\x1b_G";

struct KittyImageHeader {
    ids: Vec<u32>,
    rows: usize,
}

fn parse_kitty_image_header(line: &str) -> Option<KittyImageHeader> {
    let sequence_start = line.find(KITTY_SEQUENCE_PREFIX)?;
    let params_start = sequence_start + KITTY_SEQUENCE_PREFIX.len();
    let params_end = line[params_start..].find(';')? + params_start;

    let mut ids = Vec::new();
    let mut rows = 1;
    for param in line[params_start..params_end].split(',') {
        let Some((key, value)) = param.split_once('=') else {
            continue;
        };
        let Ok(number_value) = value.parse::<u64>() else {
            continue;
        };
        if number_value == 0 || number_value > 0xffff_ffff {
            continue;
        }
        if key == "i" {
            ids.push(number_value as u32);
        } else if key == "r" {
            rows = number_value as usize;
        }
    }
    Some(KittyImageHeader { ids, rows })
}

fn extract_kitty_image_ids(line: &str) -> Vec<u32> {
    parse_kitty_image_header(line).map_or_else(Vec::new, |header| header.ids)
}

fn extract_kitty_image_rows(line: &str) -> usize {
    parse_kitty_image_header(line).map_or(1, |header| header.rows)
}

fn is_termux_session() -> bool {
    std::env::var_os("TERMUX_VERSION").is_some_and(|value| !value.is_empty())
}

/// Capturable render state (`captureRenderState`/`restoreRenderState`).
#[derive(Debug, Clone, Default)]
pub struct TuiMainScreenRenderState {
    /// Fully rendered lines of the previous frame.
    pub previous_lines: Vec<Line>,
    /// Terminal width of the previous frame.
    pub previous_width: i64,
    /// Terminal height of the previous frame.
    pub previous_height: i64,
    /// Logical end of the content.
    pub cursor_row: usize,
    /// Actual terminal cursor row.
    pub hardware_cursor_row: usize,
    /// High-water mark of rendered lines.
    pub max_lines_rendered: usize,
    /// Index of the topmost visible line.
    pub previous_viewport_top: usize,
}

/// TUI implementation that renders into the terminal's main screen and scrollback.
pub struct TuiMainScreen {
    core: TuiCore,
    previous_lines: Vec<Line>,
    previous_kitty_image_ids: BTreeSet<u32>,
    previous_width: i64,
    previous_height: i64,
    cursor_row: usize,
    hardware_cursor_row: usize,
    max_lines_rendered: usize,
    previous_viewport_top: usize,
    full_redraw_count: usize,
}

impl TuiMainScreen {
    /// New renderer for `terminal`.
    pub fn new(terminal: Box<dyn crate::terminal::Terminal>) -> Self {
        Self::with_options(terminal, None, None)
    }

    pub fn with_options(
        terminal: Box<dyn crate::terminal::Terminal>,
        show_hardware_cursor: Option<bool>,
        log_directory: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            core: TuiCore::with_options(terminal, show_hardware_cursor, log_directory),
            previous_lines: Vec::new(),
            previous_kitty_image_ids: BTreeSet::new(),
            previous_width: 0,
            previous_height: 0,
            cursor_row: 0,
            hardware_cursor_row: 0,
            max_lines_rendered: 0,
            previous_viewport_top: 0,
            full_redraw_count: 0,
        }
    }

    /// Rendering mode of this renderer.
    pub fn mode(&self) -> TuiMode {
        TuiMode::Regular
    }

    /// Shared TUI state (children, focus, overlays, scheduling).
    pub fn core(&self) -> &TuiCore {
        &self.core
    }

    /// Number of full redraws performed so far.
    pub fn full_redraws(&self) -> usize {
        self.full_redraw_count
    }

    /// Snapshot the render state.
    pub fn capture_render_state(&self) -> TuiMainScreenRenderState {
        TuiMainScreenRenderState {
            previous_lines: self.previous_lines.clone(),
            previous_width: self.previous_width,
            previous_height: self.previous_height,
            cursor_row: self.cursor_row,
            hardware_cursor_row: self.hardware_cursor_row,
            max_lines_rendered: self.max_lines_rendered,
            previous_viewport_top: self.previous_viewport_top,
        }
    }

    /// Restore a snapshot; image lines are dropped because their transmission
    /// state does not survive the handover.
    pub fn restore_render_state(&mut self, state: TuiMainScreenRenderState) {
        self.previous_lines = state
            .previous_lines
            .iter()
            .map(|line| {
                if is_image_line(line) {
                    Line::from("")
                } else {
                    line.clone()
                }
            })
            .collect();
        self.previous_kitty_image_ids = BTreeSet::new();
        self.previous_width = state.previous_width;
        self.previous_height = state.previous_height;
        self.cursor_row = state.cursor_row;
        self.hardware_cursor_row = state.hardware_cursor_row;
        self.max_lines_rendered = state.max_lines_rendered;
        self.previous_viewport_top = state.previous_viewport_top;
    }

    fn reset_render_state(&mut self) {
        self.previous_lines = Vec::new();
        self.previous_width = -1;
        self.previous_height = -1;
        self.cursor_row = 0;
        self.hardware_cursor_row = 0;
        self.max_lines_rendered = 0;
        self.previous_viewport_top = 0;
    }

    /// Start the TUI (terminal, cursor, first frame).
    pub fn start(&mut self) {
        let core = self.core.clone();
        self.core
            .start(Box::new(move |data| core.handle_terminal_input(data)));
    }

    /// Stop the TUI and write the document into the scrollback.
    pub fn stop(&mut self, options: TuiStopOptions) {
        let animated = self.core.activity_deadline().is_some();
        self.core.set_activity_animation(false);
        if animated && !options.preserve_screen {
            self.render_now(false);
        }
        if !options.preserve_screen && !self.previous_lines.is_empty() {
            let target_row = self.previous_lines.len() as i64;
            let line_diff = target_row - self.hardware_cursor_row as i64;
            let mut buffer = String::from(" ");
            match line_diff.cmp(&0) {
                std::cmp::Ordering::Greater => buffer.push_str(&format!("\x1b[{line_diff}B")),
                std::cmp::Ordering::Less => buffer.push_str(&format!("\x1b[{}A", -line_diff)),
                std::cmp::Ordering::Equal => {}
            }
            buffer.push_str("\r\n");
            self.core.with_terminal(|terminal| terminal.write(&buffer));
        }
        self.core.stop();
    }

    /// Render immediately; `force` resets the differential state first.
    pub fn render_now(&mut self, force: bool) {
        if force {
            self.reset_render_state();
        }
        if !self.core.begin_frame() {
            return;
        }
        self.do_render();
    }

    /// Request a frame; `force` resets the differential state and preempts the throttle.
    pub fn request_render(&mut self, force: bool) {
        if force {
            self.reset_render_state();
            self.core.request_immediate_render();
            return;
        }
        self.core.request_render();
    }

    /// Render the frame the render loop reported as due.
    /// Counterpart of [`TuiCore::wait_until_render_due`]: it consumes the
    /// pending request, so a loop that waits and then calls this cannot spin.
    pub fn render_pending_frame(&mut self) {
        if self.core.begin_frame() {
            self.do_render();
        }
    }

    /// Render the pending frame once its throttle deadline has passed.
    /// (or a test) drives it. Unlike [`TuiCore::wait_until_render_due`] it
    /// returns right away when no frame is pending.
    pub async fn wait_for_render(&mut self) {
        let Some(deadline) = self.core.render_deadline() else {
            return;
        };
        let now = std::time::Instant::now();
        if deadline > now {
            tokio::time::sleep(deadline - now).await;
        }
        if self.core.begin_frame() {
            self.do_render();
        }
    }

    fn collect_kitty_image_ids(lines: &[Line]) -> BTreeSet<u32> {
        let mut ids = BTreeSet::new();
        for line in lines {
            ids.extend(extract_kitty_image_ids(line));
        }
        ids
    }

    fn delete_kitty_images(ids: impl IntoIterator<Item = u32>) -> String {
        ids.into_iter().map(delete_kitty_image).collect()
    }

    fn kitty_image_reserved_rows(lines: &[Line], index: usize, max_index: usize) -> usize {
        let rows = extract_kitty_image_rows(lines.get(index).map_or("", |line| line.as_ref()));
        if rows <= 1 {
            return 1;
        }
        let max_rows = rows
            .min(max_index.saturating_sub(index) + 1)
            .min(lines.len() - index);
        let mut reserved_rows = 1;
        while reserved_rows < max_rows {
            let line = lines
                .get(index + reserved_rows)
                .map_or("", |line| line.as_ref());
            if is_image_line(line) || visible_width(line) > 0 {
                break;
            }
            reserved_rows += 1;
        }
        reserved_rows
    }

    fn expand_changed_range_for_kitty_images(
        &self,
        first_changed: usize,
        last_changed: usize,
        new_lines: &[Line],
    ) -> (usize, usize) {
        let mut expanded_first = first_changed;
        let mut expanded_last = last_changed;
        for lines in [&self.previous_lines, &new_lines.to_vec()] {
            for index in 0..lines.len() {
                if extract_kitty_image_ids(&lines[index]).is_empty() {
                    continue;
                }
                let block_end =
                    index + Self::kitty_image_reserved_rows(lines, index, lines.len() - 1) - 1;
                if index >= first_changed || (index <= last_changed && block_end >= first_changed) {
                    expanded_first = expanded_first.min(index);
                    expanded_last = expanded_last.max(block_end);
                }
            }
        }
        (expanded_first, expanded_last)
    }

    fn delete_changed_kitty_images(&self, first_changed: usize, last_changed: usize) -> String {
        if last_changed < first_changed {
            return String::new();
        }
        let mut ids = BTreeSet::new();
        let max_line = last_changed.min(self.previous_lines.len().saturating_sub(1));
        for index in first_changed..=max_line {
            if let Some(line) = self.previous_lines.get(index) {
                ids.extend(extract_kitty_image_ids(line));
            }
        }
        Self::delete_kitty_images(ids)
    }

    fn log_redraw(&self, reason: &str, previous: usize, new: usize, height: usize) {
        if std::env::var("NOTAGENT_DEBUG_REDRAW").as_deref() != Ok("1") {
            return;
        }
        let log_path = self.core.log_directory().join("notagent-debug.log");
        let message =
            format!("fullRender: {reason} (prev={previous}, new={new}, height={height})\n");
        if let Some(parent) = log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            let _ = file.write_all(message.as_bytes());
        }
    }

    fn full_render(
        &mut self,
        clear: bool,
        new_lines: Vec<Line>,
        cursor_pos: Option<(usize, usize)>,
        width: usize,
        height: usize,
    ) {
        self.full_redraw_count += 1;
        let mut buffer = String::from("\x1b[?2026h"); // Begin synchronized output
        if clear {
            buffer.push_str(&Self::delete_kitty_images(
                self.previous_kitty_image_ids.iter().copied(),
            ));
            // Clear screen, home, then clear scrollback.
            buffer.push_str("\x1b[2J\x1b[H\x1b[3J");
        }
        let mut index = 0;
        while index < new_lines.len() {
            if index > 0 {
                buffer.push_str("\r\n");
            }
            let line = &new_lines[index];
            let image_reserved_rows = if is_image_line(line) {
                Self::kitty_image_reserved_rows(&new_lines, index, new_lines.len() - 1)
            } else {
                1
            };
            if image_reserved_rows > 1 && image_reserved_rows <= height {
                for _ in 1..image_reserved_rows {
                    buffer.push_str("\r\n");
                }
                buffer.push_str(&format!("\x1b[{}A", image_reserved_rows - 1));
                buffer.push_str(line);
                buffer.push_str(&format!("\x1b[{}B", image_reserved_rows - 1));
                index += image_reserved_rows;
                continue;
            }
            buffer.push_str(line);
            index += 1;
        }
        buffer.push_str("\x1b[?2026l"); // End synchronized output
        self.core.with_terminal(|terminal| terminal.write(&buffer));

        self.cursor_row = new_lines.len().saturating_sub(1);
        self.hardware_cursor_row = self.cursor_row;
        // Reset the high-water mark when clearing, otherwise track growth.
        self.max_lines_rendered = if clear {
            new_lines.len()
        } else {
            self.max_lines_rendered.max(new_lines.len())
        };
        let buffer_length = height.max(new_lines.len());
        self.previous_viewport_top = buffer_length.saturating_sub(height);
        self.position_hardware_cursor(cursor_pos, new_lines.len());
        self.previous_kitty_image_ids = Self::collect_kitty_image_ids(&new_lines);
        self.previous_lines = new_lines;
        self.previous_width = width as i64;
        self.previous_height = height as i64;
    }

    fn do_render(&mut self) {
        if self.core.is_stopped() {
            return;
        }
        let width = self.core.columns();
        let height = self.core.rows();
        let width_changed = self.previous_width != 0 && self.previous_width != width as i64;
        let height_changed = self.previous_height != 0 && self.previous_height != height as i64;
        let previous_buffer_length = if self.previous_height > 0 {
            self.previous_viewport_top + self.previous_height as usize
        } else {
            height
        };
        let mut prev_viewport_top = if height_changed {
            previous_buffer_length.saturating_sub(height)
        } else {
            self.previous_viewport_top
        };
        let mut viewport_top = prev_viewport_top;
        let mut hardware_cursor_row = self.hardware_cursor_row as i64;

        // Render all components to get the new lines.
        let mut new_lines = self.core.render_children(width);

        // Composite overlays before the differential compare.
        if self.core.has_overlay_entries() {
            new_lines = self.core.composite_overlays(new_lines, width, height);
        }

        let first_visible = new_lines.len().saturating_sub(height);
        self.core
            .paint_activity(&mut new_lines, first_visible, &self.previous_lines);

        // Extract the cursor position before the line resets (the marker must be
        // found first).
        let cursor_pos = self.core.extract_cursor_position(&mut new_lines, height);
        self.core.apply_line_resets(&mut new_lines);

        // First render — output everything without clearing (assumes a clean screen).
        if self.previous_lines.is_empty() && !width_changed && !height_changed {
            self.log_redraw("first render", 0, new_lines.len(), height);
            self.full_render(false, new_lines, cursor_pos, width, height);
            return;
        }

        // Width changes always need a full redraw because wrapping changes.
        if width_changed {
            self.log_redraw(
                &format!(
                    "terminal width changed ({} -> {width})",
                    self.previous_width
                ),
                self.previous_lines.len(),
                new_lines.len(),
                height,
            );
            self.full_render(true, new_lines, cursor_pos, width, height);
            return;
        }

        // Height changes normally need a full redraw to keep the viewport
        // aligned, but Termux toggles height with its software keyboard, where a
        // full redraw would replay the whole history on every toggle.
        if height_changed && !is_termux_session() {
            self.log_redraw(
                &format!(
                    "terminal height changed ({} -> {height})",
                    self.previous_height
                ),
                self.previous_lines.len(),
                new_lines.len(),
                height,
            );
            self.full_render(true, new_lines, cursor_pos, width, height);
            return;
        }

        // Content shrank below the working area and there are no overlays —
        // redraw to clear the empty rows (overlays need the padding).
        if self.core.get_clear_on_shrink()
            && new_lines.len() < self.max_lines_rendered
            && !self.core.has_overlay_entries()
        {
            self.log_redraw(
                &format!(
                    "clearOnShrink (maxLinesRendered={})",
                    self.max_lines_rendered
                ),
                self.previous_lines.len(),
                new_lines.len(),
                height,
            );
            self.full_render(true, new_lines, cursor_pos, width, height);
            return;
        }

        // Find the first and last changed line. Pointer identity settles a
        // shared, unchanged line without reading its bytes; the content
        // comparison stays as the fallback, so a line that lost its identity
        // is slower, never wrong. A missing line still compares as "".
        let mut first_changed: Option<usize> = None;
        let mut last_changed: usize = 0;
        let max_lines = new_lines.len().max(self.previous_lines.len());
        for index in 0..max_lines {
            let old_line = self.previous_lines.get(index);
            let new_line = new_lines.get(index);
            let unchanged = match (old_line, new_line) {
                (Some(old), Some(new)) => Line::ptr_eq(old, new) || old == new,
                _ => {
                    old_line.map_or("", |line| line.as_ref())
                        == new_line.map_or("", |line| line.as_ref())
                }
            };
            if !unchanged {
                if first_changed.is_none() {
                    first_changed = Some(index);
                }
                last_changed = index;
            }
        }
        let appended_lines = new_lines.len() > self.previous_lines.len();
        if appended_lines {
            if first_changed.is_none() {
                first_changed = Some(self.previous_lines.len());
            }
            last_changed = new_lines.len() - 1;
        }
        if let Some(first) = first_changed {
            let (expanded_first, expanded_last) =
                self.expand_changed_range_for_kitty_images(first, last_changed, &new_lines);
            first_changed = Some(expanded_first);
            last_changed = expanded_last;
        }
        let append_start = appended_lines
            && first_changed == Some(self.previous_lines.len())
            && first_changed.is_some_and(|first| first > 0);

        // No changes — but the hardware cursor may still have moved.
        let Some(first_changed) = first_changed else {
            self.position_hardware_cursor(cursor_pos, new_lines.len());
            self.previous_viewport_top = prev_viewport_top;
            self.previous_height = height as i64;
            return;
        };

        let compute_line_diff =
            |target_row: usize, hardware_cursor_row: i64, prev_top: usize, top: usize| -> i64 {
                let current_screen_row = hardware_cursor_row - prev_top as i64;
                let target_screen_row = target_row as i64 - top as i64;
                target_screen_row - current_screen_row
            };

        // All changes are in deleted lines (nothing to render, just clear).
        if first_changed >= new_lines.len() {
            if self.previous_lines.len() > new_lines.len() {
                let mut buffer = String::from("\x1b[?2026h");
                buffer.push_str(&self.delete_changed_kitty_images(first_changed, last_changed));
                let target_row = new_lines.len().saturating_sub(1);
                if target_row < prev_viewport_top {
                    self.log_redraw(
                        &format!(
                            "deleted lines moved viewport up ({target_row} < {prev_viewport_top})"
                        ),
                        self.previous_lines.len(),
                        new_lines.len(),
                        height,
                    );
                    self.full_render(true, new_lines, cursor_pos, width, height);
                    return;
                }
                let line_diff = compute_line_diff(
                    target_row,
                    hardware_cursor_row,
                    prev_viewport_top,
                    viewport_top,
                );
                match line_diff.cmp(&0) {
                    std::cmp::Ordering::Greater => buffer.push_str(&format!("\x1b[{line_diff}B")),
                    std::cmp::Ordering::Less => buffer.push_str(&format!("\x1b[{}A", -line_diff)),
                    std::cmp::Ordering::Equal => {}
                }
                buffer.push('\r');

                let extra_lines = self.previous_lines.len() - new_lines.len();
                if extra_lines > height {
                    self.log_redraw(
                        &format!("extraLines > height ({extra_lines} > {height})"),
                        self.previous_lines.len(),
                        new_lines.len(),
                        height,
                    );
                    self.full_render(true, new_lines, cursor_pos, width, height);
                    return;
                }
                let clear_start_offset = usize::from(!new_lines.is_empty());
                if extra_lines > 0 && clear_start_offset > 0 {
                    buffer.push_str(&format!("\x1b[{clear_start_offset}B"));
                }
                for index in 0..extra_lines {
                    buffer.push_str("\r\x1b[2K");
                    if index < extra_lines - 1 {
                        buffer.push_str("\x1b[1B");
                    }
                }
                let move_back = (extra_lines + clear_start_offset).saturating_sub(1);
                if move_back > 0 {
                    buffer.push_str(&format!("\x1b[{move_back}A"));
                }
                buffer.push_str("\x1b[?2026l");
                self.core.with_terminal(|terminal| terminal.write(&buffer));
                self.cursor_row = target_row;
                self.hardware_cursor_row = target_row;
            }
            self.position_hardware_cursor(cursor_pos, new_lines.len());
            self.previous_kitty_image_ids = Self::collect_kitty_image_ids(&new_lines);
            self.previous_lines = new_lines;
            self.previous_width = width as i64;
            self.previous_height = height as i64;
            self.previous_viewport_top = prev_viewport_top;
            return;
        }

        // Differential rendering can only touch what was actually visible.
        if first_changed < prev_viewport_top {
            self.log_redraw(
                &format!("firstChanged < viewportTop ({first_changed} < {prev_viewport_top})"),
                self.previous_lines.len(),
                new_lines.len(),
                height,
            );
            self.full_render(true, new_lines, cursor_pos, width, height);
            return;
        }

        // Build one buffer with all updates, wrapped in synchronized output.
        let mut buffer = String::from("\x1b[?2026h");
        buffer.push_str(&self.delete_changed_kitty_images(first_changed, last_changed));
        let prev_viewport_bottom = prev_viewport_top + height - 1;
        let move_target_row = if append_start {
            first_changed - 1
        } else {
            first_changed
        };
        if move_target_row > prev_viewport_bottom {
            let current_screen_row =
                (hardware_cursor_row - prev_viewport_top as i64).clamp(0, height as i64 - 1);
            let move_to_bottom = height as i64 - 1 - current_screen_row;
            if move_to_bottom > 0 {
                buffer.push_str(&format!("\x1b[{move_to_bottom}B"));
            }
            let scroll = move_target_row - prev_viewport_bottom;
            buffer.push_str(&"\r\n".repeat(scroll));
            prev_viewport_top += scroll;
            viewport_top += scroll;
            hardware_cursor_row = move_target_row as i64;
        }

        // Move the cursor to the first changed line.
        let line_diff = compute_line_diff(
            move_target_row,
            hardware_cursor_row,
            prev_viewport_top,
            viewport_top,
        );
        match line_diff.cmp(&0) {
            std::cmp::Ordering::Greater => buffer.push_str(&format!("\x1b[{line_diff}B")),
            std::cmp::Ordering::Less => buffer.push_str(&format!("\x1b[{}A", -line_diff)),
            std::cmp::Ordering::Equal => {}
        }
        buffer.push_str(if append_start { "\r\n" } else { "\r" });

        // Only render the changed lines, which keeps single-line updates (e.g. a
        // spinner) from flickering the rest of the screen.
        let render_end = last_changed.min(new_lines.len() - 1);
        let mut index = first_changed;
        while index <= render_end {
            if index > first_changed {
                buffer.push_str("\r\n");
            }
            let line = new_lines[index].clone();
            let is_image = is_image_line(&line);
            let image_reserved_rows = if is_image {
                Self::kitty_image_reserved_rows(&new_lines, index, render_end)
            } else {
                1
            };
            if image_reserved_rows > 1 {
                let image_start_screen_row = index as i64 - viewport_top as i64;
                if image_start_screen_row < 0
                    || image_start_screen_row + image_reserved_rows as i64 > height as i64
                {
                    self.log_redraw(
                        &format!(
                            "kitty image pre-clear would scroll ({image_start_screen_row} + {image_reserved_rows} > {height})"
                        ),
                        self.previous_lines.len(),
                        new_lines.len(),
                        height,
                    );
                    self.full_render(true, new_lines, cursor_pos, width, height);
                    return;
                }

                buffer.push_str("\x1b[2K");
                for _ in 1..image_reserved_rows {
                    buffer.push_str("\r\n\x1b[2K");
                }
                buffer.push_str(&format!("\x1b[{}A", image_reserved_rows - 1));
                buffer.push_str(&line);
                buffer.push_str(&format!("\x1b[{}B", image_reserved_rows - 1));
                index += image_reserved_rows;
                continue;
            }

            buffer.push_str("\x1b[2K"); // Clear current line
            if !is_image && visible_width(&line) > width {
                self.crash_on_overwide_line(index, &line, &new_lines, width);
            }
            buffer.push_str(&line);
            index += 1;
        }

        // Track where the cursor ended up after rendering.
        let mut final_cursor_row = render_end;

        // If there were more lines before, clear them and move the cursor back.
        if self.previous_lines.len() > new_lines.len() {
            if render_end < new_lines.len() - 1 {
                let move_down = new_lines.len() - 1 - render_end;
                buffer.push_str(&format!("\x1b[{move_down}B"));
                final_cursor_row = new_lines.len() - 1;
            }
            let extra_lines = self.previous_lines.len() - new_lines.len();
            for _ in new_lines.len()..self.previous_lines.len() {
                buffer.push_str("\r\n\x1b[2K");
            }
            buffer.push_str(&format!("\x1b[{extra_lines}A"));
        }

        buffer.push_str("\x1b[?2026l"); // End synchronized output
        self.write_frame_debug_log(
            first_changed,
            viewport_top,
            height,
            line_diff,
            hardware_cursor_row,
            render_end,
            final_cursor_row,
            cursor_pos,
            &new_lines,
            &buffer,
        );

        self.core.with_terminal(|terminal| terminal.write(&buffer));

        // cursorRow tracks the end of the content (for viewport calculation),
        // hardwareCursorRow the actual terminal cursor position.
        self.cursor_row = new_lines.len().saturating_sub(1);
        self.hardware_cursor_row = final_cursor_row;
        self.max_lines_rendered = self.max_lines_rendered.max(new_lines.len());
        self.previous_viewport_top =
            prev_viewport_top.max((final_cursor_row as i64 - height as i64 + 1).max(0) as usize);

        self.position_hardware_cursor(cursor_pos, new_lines.len());

        self.previous_kitty_image_ids = Self::collect_kitty_image_ids(&new_lines);
        self.previous_lines = new_lines;
        self.previous_width = width as i64;
        self.previous_height = height as i64;
    }

    /// Write the crash log, stop the TUI and panic — a component that does not
    fn crash_on_overwide_line(
        &mut self,
        index: usize,
        line: &str,
        new_lines: &[Line],
        width: usize,
    ) -> ! {
        let crash_log_path = self.core.log_directory().join("notagent-crash.log");
        let mut crash_data = vec![
            "Crash".to_string(),
            format!("Terminal width: {width}"),
            format!("Line {index} visible width: {}", visible_width(line)),
            String::new(),
            "=== All rendered lines ===".to_string(),
        ];
        for (idx, l) in new_lines.iter().enumerate() {
            crash_data.push(format!("[{idx}] (w={}) {l}", visible_width(l)));
        }
        crash_data.push(String::new());
        if let Some(parent) = crash_log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&crash_log_path, crash_data.join("\n"));

        // Clean up terminal state before failing.
        self.core.stop();

        panic!(
            "Rendered line {index} exceeds terminal width ({} > {width}).\n\n\
             This is likely caused by a custom TUI component not truncating its output.\n\
             Use visibleWidth() to measure and truncateToWidth() to truncate lines.\n\n\
             Debug log written to: {}",
            visible_width(line),
            crash_log_path.display()
        );
    }

    #[allow(clippy::too_many_arguments)] // The dump keeps each diagnostic field explicit.
    fn write_frame_debug_log(
        &self,
        first_changed: usize,
        viewport_top: usize,
        height: usize,
        line_diff: i64,
        hardware_cursor_row: i64,
        render_end: usize,
        final_cursor_row: usize,
        cursor_pos: Option<(usize, usize)>,
        new_lines: &[Line],
        buffer: &str,
    ) {
        if std::env::var("NOTAGENT_TUI_DEBUG").as_deref() != Ok("1") {
            return;
        }
        let debug_dir = std::path::Path::new("/tmp/tui");
        let _ = std::fs::create_dir_all(debug_dir);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let debug_path = debug_dir.join(format!("render-{stamp}-{}.log", std::process::id()));
        let debug_data = [
            format!("firstChanged: {first_changed}"),
            format!("viewportTop: {viewport_top}"),
            format!("cursorRow: {}", self.cursor_row),
            format!("height: {height}"),
            format!("lineDiff: {line_diff}"),
            format!("hardwareCursorRow: {hardware_cursor_row}"),
            format!("renderEnd: {render_end}"),
            format!("finalCursorRow: {final_cursor_row}"),
            format!("cursorPos: {cursor_pos:?}"),
            format!("newLines.length: {}", new_lines.len()),
            format!("previousLines.length: {}", self.previous_lines.len()),
            String::new(),
            "=== newLines ===".to_string(),
            format!("{new_lines:#?}"),
            String::new(),
            "=== previousLines ===".to_string(),
            format!("{:#?}", self.previous_lines),
            String::new(),
            "=== buffer ===".to_string(),
            format!("{buffer:?}"),
        ]
        .join("\n");
        let _ = std::fs::write(debug_path, debug_data);
    }

    /// Position the hardware cursor for the IME candidate window.
    fn position_hardware_cursor(&mut self, cursor_pos: Option<(usize, usize)>, total_lines: usize) {
        if total_lines == 0 {
            self.core.with_terminal(|terminal| terminal.hide_cursor());
            return;
        }

        // Where the cursor goes when no component claims it — a dialog is open,
        // say, and nothing is emitting the marker.
        //
        // Hiding it is not enough on its own. A partial repaint leaves the
        // physical cursor at the end of whatever it just painted, and a terminal
        // that draws its cursor regardless of `?25l` then shows it hopping
        // between the regions that repaint on their own timers. Parking it
        // somewhere fixed costs one escape sequence and makes the frame look the
        // same either way.
        let (row, col) = cursor_pos.unwrap_or((total_lines - 1, 0));

        let target_row = row.min(total_lines - 1);
        let target_col = col;

        let row_delta = target_row as i64 - self.hardware_cursor_row as i64;
        let mut buffer = String::new();
        match row_delta.cmp(&0) {
            std::cmp::Ordering::Greater => buffer.push_str(&format!("\x1b[{row_delta}B")),
            std::cmp::Ordering::Less => buffer.push_str(&format!("\x1b[{}A", -row_delta)),
            std::cmp::Ordering::Equal => {}
        }
        // Move to the absolute column (1-indexed).
        buffer.push_str(&format!("\x1b[{}G", target_col + 1));

        self.core.with_terminal(|terminal| terminal.write(&buffer));

        self.hardware_cursor_row = target_row;
        // Only a component that asked for the cursor gets a visible one.
        let show = cursor_pos.is_some() && self.core.get_show_hardware_cursor();
        self.core.with_terminal(|terminal| {
            if show {
                terminal.show_cursor();
            } else {
                terminal.hide_cursor();
            }
        });
    }
}

impl crate::tui::RenderLoop for TuiMainScreen {
    fn core(&self) -> &TuiCore {
        &self.core
    }

    fn render_pending_frame(&mut self) {
        TuiMainScreen::render_pending_frame(self);
    }
}
