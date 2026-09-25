//! Retains source-backed entries while regular frames emit settled history once.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::ops::Deref;
use std::rc::Rc;

use crate::terminal_image::get_kitty_image_metadata;
use crate::tui::{
    Component, ComponentRef, Container, HistoryRegions, Line, TranscriptAnchor, WindowedContent,
};

/// Replay budget for a terminal whose scrollback size is unknown.
pub const REGULAR_REPLAY_ROWS: usize = 1_000;

/// Rows to replay into native scrollback on a rebuild. The values follow the
/// documented scrollback defaults of terminals that can be identified:
/// replaying more rows than the terminal retains costs time on every rebuild
/// without giving the user more history.
pub fn regular_replay_rows() -> usize {
    regular_replay_rows_from(&|name| std::env::var(name).ok())
}

/// [`regular_replay_rows`] with an injectable environment. The probe order
/// matters: `TERM_PROGRAM` names the terminal unless it is tmux, and an earlier
/// terminal-specific variable masks a later one.
pub fn regular_replay_rows_from(env: &dyn Fn(&str) -> Option<String>) -> usize {
    const VSCODE: usize = 1_000;
    const WINDOWS_TERMINAL: usize = 9_001;
    const WEZTERM: usize = 3_500;
    const ALACRITTY: usize = 10_000;

    let non_empty = |name: &str| env(name).filter(|value| !value.is_empty());
    if let Some(program) = non_empty("TERM_PROGRAM")
        && !program.eq_ignore_ascii_case("tmux")
    {
        let normalized: String = program
            .trim()
            .chars()
            .filter(|character| !matches!(character, ' ' | '-' | '_' | '.'))
            .map(|character| character.to_ascii_lowercase())
            .collect();
        return match normalized.as_str() {
            "vscode" => VSCODE,
            "wezterm" => WEZTERM,
            "alacritty" => ALACRITTY,
            "windowsterminal" => WINDOWS_TERMINAL,
            _ => REGULAR_REPLAY_ROWS,
        };
    }
    if non_empty("GHOSTTY_RESOURCES_DIR").is_some() {
        return REGULAR_REPLAY_ROWS;
    }
    if env("WEZTERM_VERSION").is_some() {
        return WEZTERM;
    }
    let term = env("TERM").unwrap_or_default();
    if [
        "ITERM_SESSION_ID",
        "ITERM_PROFILE",
        "ITERM_PROFILE_NAME",
        "TERM_SESSION_ID",
    ]
    .iter()
    .any(|name| env(name).is_some())
        || env("KITTY_WINDOW_ID").is_some()
        || term.contains("kitty")
    {
        return REGULAR_REPLAY_ROWS;
    }
    if env("ALACRITTY_SOCKET").is_some() || term == "alacritty" {
        return ALACRITTY;
    }
    if ["KONSOLE_VERSION", "GNOME_TERMINAL_SCREEN", "VTE_VERSION"]
        .iter()
        .any(|name| env(name).is_some())
    {
        return REGULAR_REPLAY_ROWS;
    }
    if env("WT_SESSION").is_some() {
        return WINDOWS_TERMINAL;
    }
    REGULAR_REPLAY_ROWS
}

#[derive(Default)]
struct HeightIndex {
    tree: Vec<usize>,
}

impl HeightIndex {
    fn from_heights(heights: impl IntoIterator<Item = usize>) -> Self {
        let mut index = Self { tree: vec![0] };
        for height in heights {
            let next = index.tree.len();
            let span = next.isolate_lowest_one();
            let previous = index.prefix(next - 1) - index.prefix(next - span);
            index.tree.push(previous + height);
        }
        index
    }

    fn prefix(&self, mut count: usize) -> usize {
        let mut total = 0;
        while count > 0 {
            total += self.tree[count];
            count &= count - 1;
        }
        total
    }

    fn total(&self) -> usize {
        self.prefix(self.tree.len().saturating_sub(1))
    }

    fn push(&mut self, height: usize) {
        if self.tree.is_empty() {
            self.tree.push(0);
        }
        let next = self.tree.len();
        let span = next.isolate_lowest_one();
        let previous = self.prefix(next - 1) - self.prefix(next - span);
        self.tree.push(previous + height);
    }

    fn set(&mut self, entry: usize, old: usize, new: usize) {
        let mut index = entry + 1;
        while index < self.tree.len() {
            if new >= old {
                self.tree[index] += new - old;
            } else {
                self.tree[index] -= old - new;
            }
            index += index.isolate_lowest_one();
        }
    }

    fn entry_at(&self, row: usize) -> usize {
        let mut index = 0;
        let mut sum = 0;
        let mut step = self.tree.len().next_power_of_two() / 2;
        while step > 0 {
            let next = index + step;
            if next < self.tree.len() && sum + self.tree[next] <= row {
                index = next;
                sum += self.tree[next];
            }
            step /= 2;
        }
        index
    }
}

struct Entry {
    id: u64,
    content_revision: u64,
    presentation_revision: u64,
    stable: bool,
    cached: Option<(usize, Vec<Line>)>,
    /// Height at `indexed_width`, exactly the value the height index holds.
    /// Regular frames render at their own width and must not write it: the
    /// index would then disagree with the height its next update starts from.
    last_height: usize,
}

#[derive(Clone, Copy, Default)]
pub struct TranscriptRenderCounters {
    pub stable_entries: usize,
    pub active_entries: usize,
    pub fullscreen_entries: usize,
    pub height_updates: usize,
}

/// A transcript with a native-history cursor and an indexed fullscreen view.
pub struct TranscriptContainer {
    inner: Container,
    entries: Vec<Entry>,
    regular: bool,
    first_regular_entry: usize,
    first_regular_offset: usize,
    history_emitted_entry: usize,
    regular_width: Option<usize>,
    indexed_width: Option<usize>,
    heights: HeightIndex,
    dirty: BTreeSet<usize>,
    active_candidates: BTreeSet<usize>,
    window_cached: Vec<usize>,
    revision: u64,
    replay_budget: Option<usize>,
    transient_status: Option<ComponentRef>,
    next_id: u64,
    index_by_component: HashMap<usize, usize>,
    external_changes: Rc<RefCell<Vec<usize>>>,
    history_reflow_needed: bool,
    /// Rows a still-mutable entry already streamed into native history,
    /// keyed by its id. Once the entry settles, only the rows after them are
    /// emitted, provided its final render still starts with them.
    stream_emitted: Option<StreamEmitted>,
    counters: TranscriptRenderCounters,
}

struct StreamEmitted {
    id: u64,
    rows: Vec<Line>,
    /// How many rows at the top of the entry's current render already left
    /// the screen into history. The entry's own commits cover the same rows
    /// again, so they are skipped there instead of written twice.
    overflow: usize,
}

impl Default for TranscriptContainer {
    fn default() -> Self {
        Self {
            inner: Container::new(),
            entries: Vec::new(),
            regular: false,
            first_regular_entry: 0,
            first_regular_offset: 0,
            history_emitted_entry: 0,
            regular_width: None,
            indexed_width: None,
            heights: HeightIndex::default(),
            dirty: BTreeSet::new(),
            active_candidates: BTreeSet::new(),
            window_cached: Vec::new(),
            revision: 0,
            replay_budget: None,
            transient_status: None,
            next_id: 1,
            index_by_component: HashMap::new(),
            external_changes: Rc::new(RefCell::new(Vec::new())),
            history_reflow_needed: false,
            stream_emitted: None,
            counters: TranscriptRenderCounters::default(),
        }
    }
}

/// Semantic zone marks (OSC 133) are invisible and depend on whether the
/// finished message turned out to call tools, so they do not make an already
/// streamed row differ from its final form.
fn without_semantic_zones(line: &str) -> std::borrow::Cow<'_, str> {
    const START: &str = "\x1b]133;";
    if !line.contains(START) {
        return std::borrow::Cow::Borrowed(line);
    }
    let mut result = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(index) = rest.find(START) {
        result.push_str(&rest[..index]);
        let after = &rest[index..];
        match after.find('\x07') {
            Some(end) => rest = &after[end + 1..],
            None => {
                rest = after;
                break;
            }
        }
    }
    result.push_str(rest);
    std::borrow::Cow::Owned(result)
}

impl TranscriptContainer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_regular(&mut self, regular: bool) {
        self.regular = regular;
        if !regular {
            self.regular_width = None;
        }
    }

    pub fn set_replay_budget(&mut self, rows: usize) {
        self.replay_budget = Some(rows);
    }

    fn replay_budget(&self) -> usize {
        self.replay_budget.unwrap_or(REGULAR_REPLAY_ROWS)
    }

    /// Re-expose the recent tail within the configured replay budget.
    pub fn replay_recent(&mut self, width: usize) {
        self.replay_recent_rows(width, self.replay_budget());
    }

    /// A callback can announce a change without borrowing the store during a render.
    pub fn external_change_sender(&self) -> Rc<dyn Fn(usize)> {
        let queue = Rc::clone(&self.external_changes);
        Rc::new(move |component_key| queue.borrow_mut().push(component_key))
    }

    pub fn render_counters(&self) -> TranscriptRenderCounters {
        self.counters
    }

    fn drain_external_changes(&mut self) {
        let changed = std::mem::take(&mut *self.external_changes.borrow_mut());
        for key in changed {
            if let Some(&index) = self.index_by_component.get(&key) {
                self.note_changed(index);
            }
        }
    }

    fn note_changed(&mut self, index: usize) {
        self.revision = self.revision.wrapping_add(1);
        if index < self.history_emitted_entry {
            self.history_reflow_needed = true;
        }
        let entry = &mut self.entries[index];
        entry.content_revision = entry.content_revision.wrapping_add(1);
        entry.cached = None;
        self.dirty.insert(index);
        self.active_candidates.insert(index);
    }

    pub fn add_child(&mut self, component: ComponentRef) {
        if let Some(previous) = self.transient_status.take() {
            self.mark_stable(&previous);
        }
        self.revision = self.revision.wrapping_add(1);
        let regular_height = self
            .regular_width
            .map_or(0, |width| component.borrow_mut().render(width).len());
        let key = Rc::as_ptr(&component).cast::<()>() as usize;
        self.index_by_component.insert(key, self.entries.len());
        self.inner.add_child(component);
        // The entry enters the index empty; the dirty mark measures it at
        // the indexed width, which may differ from the regular one.
        self.entries.push(Entry {
            id: self.next_id,
            content_revision: 0,
            presentation_revision: 0,
            stable: true,
            cached: None,
            last_height: 0,
        });
        self.next_id = self.next_id.wrapping_add(1);
        self.dirty.insert(self.entries.len() - 1);
        if regular_height > 0 || self.regular_width.is_none() {
            self.active_candidates.insert(self.entries.len() - 1);
        }
        if self.indexed_width.is_some() {
            self.heights.push(0);
        }
    }

    pub fn remove_child(&mut self, component: &ComponentRef) {
        if self
            .transient_status
            .as_ref()
            .is_some_and(|status| Rc::ptr_eq(status, component))
        {
            self.transient_status = None;
        }
        if let Some(index) = self
            .index_by_component
            .remove(&(Rc::as_ptr(component).cast::<()>() as usize))
        {
            self.revision = self.revision.wrapping_add(1);
            self.inner.remove_child(component);
            let removed = self.entries.remove(index);
            if index < self.history_emitted_entry {
                self.history_reflow_needed = true;
            }
            if self
                .stream_emitted
                .as_ref()
                .is_some_and(|emitted| emitted.id == removed.id)
            {
                // Its streamed rows are in scrollback without their entry.
                self.stream_emitted = None;
                self.history_reflow_needed = true;
            }
            self.rebuild_component_index();
            self.dirty = self
                .dirty
                .iter()
                .filter_map(|dirty| {
                    (*dirty != index).then_some(if *dirty > index { dirty - 1 } else { *dirty })
                })
                .collect();
            self.active_candidates = self
                .active_candidates
                .iter()
                .filter_map(|candidate| {
                    (*candidate != index).then_some(if *candidate > index {
                        candidate - 1
                    } else {
                        *candidate
                    })
                })
                .collect();
            self.window_cached.clear();
            self.rebuild_heights();
            if index < self.first_regular_entry {
                self.first_regular_entry -= 1;
            } else if index == self.first_regular_entry {
                self.first_regular_offset = 0;
            }
            if index < self.history_emitted_entry {
                self.history_emitted_entry -= 1;
            }
        }
    }

    pub fn clear(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.inner.clear();
        self.entries.clear();
        self.first_regular_entry = 0;
        self.first_regular_offset = 0;
        self.history_emitted_entry = 0;
        self.regular_width = None;
        self.indexed_width = None;
        self.heights = HeightIndex::default();
        self.dirty.clear();
        self.active_candidates.clear();
        self.window_cached.clear();
        self.transient_status = None;
        self.index_by_component.clear();
        self.external_changes.borrow_mut().clear();
        self.history_reflow_needed = true;
        self.stream_emitted = None;
    }

    /// A live cell remains in the viewport until its final appearance is known.
    pub fn mark_mutable(&mut self, component: &ComponentRef) {
        self.set_stable(component, false);
    }

    pub fn mark_stable(&mut self, component: &ComponentRef) {
        if self
            .transient_status
            .as_ref()
            .is_some_and(|status| Rc::ptr_eq(status, component))
        {
            self.transient_status = None;
        }
        self.set_stable(component, true);
    }

    pub fn mark_transient_status(&mut self, component: &ComponentRef) {
        self.mark_mutable(component);
        self.transient_status = Some(Rc::clone(component));
    }

    fn set_stable(&mut self, component: &ComponentRef, stable: bool) {
        if let Some(&index) = self
            .index_by_component
            .get(&(Rc::as_ptr(component).cast::<()>() as usize))
        {
            self.revision = self.revision.wrapping_add(1);
            if index < self.history_emitted_entry {
                self.history_reflow_needed = true;
            }
            let entry = &mut self.entries[index];
            entry.stable = stable;
            entry.presentation_revision = entry.presentation_revision.wrapping_add(1);
            entry.cached = None;
            self.dirty.insert(index);
            self.active_candidates.insert(index);
        }
    }

    /// Whether `component` is a settled entry; a settled entry may already be
    /// in native scrollback, so its owner must not change it any more.
    pub fn is_stable(&self, component: &ComponentRef) -> bool {
        self.index_by_component
            .get(&(Rc::as_ptr(component).cast::<()>() as usize))
            .is_some_and(|&index| self.entries[index].stable)
    }

    pub fn mark_changed(&mut self, component: &ComponentRef) {
        if let Some(&index) = self
            .index_by_component
            .get(&(Rc::as_ptr(component).cast::<()>() as usize))
        {
            self.note_changed(index);
        }
    }

    /// Re-expose the recent source-backed tail after a deliberate visual rebuild.
    pub fn replay_from(&mut self, first_entry: usize) {
        self.first_regular_entry = first_entry.min(self.entries.len());
        self.first_regular_offset = 0;
        self.history_emitted_entry = self.first_regular_entry;
        self.stream_emitted = None;
    }

    fn image_safe_offset(lines: &[Line], offset: usize) -> usize {
        let boundary = offset.min(lines.len());
        lines
            .iter()
            .enumerate()
            .take(boundary)
            .find_map(|(index, line)| {
                get_kitty_image_metadata(line)
                    .filter(|image| index.saturating_add(image.rows) > boundary)
                    .map(|_| index)
            })
            .unwrap_or(boundary)
    }

    /// Select complete recent entries before rebuilding terminal scrollback.
    pub fn replay_recent_rows(&mut self, width: usize, budget: usize) {
        let mut rows: usize = 0;
        let mut first = self.entries.len();
        let mut first_offset = 0;
        for index in (0..self.entries.len()).rev() {
            let entry = &self.entries[index];
            let lines = match &entry.cached {
                Some((cached_width, lines)) if *cached_width == width => lines.clone(),
                _ => self.inner.children[index].borrow_mut().render(width),
            };
            let height = lines.len();
            if height > 0 {
                self.active_candidates.insert(index);
            } else {
                self.active_candidates.remove(&index);
            }
            if rows.saturating_add(height) > budget {
                let available = budget.saturating_sub(rows);
                if available > 0 || first == self.entries.len() {
                    first = index;
                    first_offset =
                        Self::image_safe_offset(&lines, height.saturating_sub(available));
                }
                break;
            }
            rows += height;
            first = index;
        }
        self.first_regular_entry = first;
        self.first_regular_offset = first_offset;
        self.history_emitted_entry = first;
        self.regular_width = Some(width);
        self.history_reflow_needed = false;
        self.stream_emitted = None;
    }

    pub fn first_regular_entry(&self) -> usize {
        self.first_regular_entry
    }

    pub fn first_regular_offset(&self) -> usize {
        self.first_regular_offset
    }

    pub fn entry_id(&self, component: &ComponentRef) -> Option<u64> {
        self.index_by_component
            .get(&(Rc::as_ptr(component).cast::<()>() as usize))
            .map(|&index| self.entries[index].id)
    }

    pub fn entry_revisions(&self, component: &ComponentRef) -> Option<(u64, u64)> {
        self.index_by_component
            .get(&(Rc::as_ptr(component).cast::<()>() as usize))
            .map(|&index| {
                let entry = &self.entries[index];
                (entry.content_revision, entry.presentation_revision)
            })
    }

    fn stream_overflow(&self, id: u64) -> usize {
        self.stream_emitted
            .as_ref()
            .filter(|emitted| emitted.id == id)
            .map_or(0, |emitted| emitted.overflow)
    }

    /// Record `rows` of entry `id` as written to history; `overflow` of them
    /// were taken from its current render rather than committed by it.
    fn emit_stream_rows(
        &mut self,
        id: u64,
        rows: &[Line],
        overflow: usize,
        history: &mut Vec<Line>,
    ) {
        if rows.is_empty() {
            return;
        }
        match &mut self.stream_emitted {
            Some(emitted) if emitted.id == id => {
                emitted.rows.extend_from_slice(rows);
                emitted.overflow += overflow;
            }
            _ => {
                self.stream_emitted = Some(StreamEmitted {
                    id,
                    rows: rows.to_vec(),
                    overflow,
                });
            }
        }
        history.extend_from_slice(rows);
    }

    /// Whether rows already streamed for the next history entry no longer
    /// match how that entry now renders. Scrollback cannot be edited, so a
    /// mismatch can only be repaired by a replay.
    fn streamed_rows_diverged(&mut self, width: usize) -> bool {
        let index = self.history_emitted_entry;
        let Some(StreamEmitted { id, rows, .. }) = &self.stream_emitted else {
            return false;
        };
        let Some(entry) = self.entries.get(index) else {
            return true;
        };
        if entry.id != *id {
            return true;
        }
        if !entry.stable {
            return false;
        }
        let lines = match &entry.cached {
            Some((cached_width, lines)) if *cached_width == width => lines.clone(),
            _ => self.inner.children[index].borrow_mut().render(width),
        };
        let diverged = lines.len() < rows.len()
            || lines.iter().zip(rows).any(|(final_row, streamed_row)| {
                without_semantic_zones(final_row) != without_semantic_zones(streamed_row)
            });
        self.entries[index].cached = Some((width, lines));
        diverged
    }

    fn rebuild_component_index(&mut self) {
        self.index_by_component.clear();
        for (index, component) in self.inner.children.iter().enumerate() {
            self.index_by_component
                .insert(Rc::as_ptr(component).cast::<()>() as usize, index);
        }
    }

    fn rebuild_heights(&mut self) {
        self.heights =
            HeightIndex::from_heights(self.entries.iter().map(|entry| entry.last_height));
    }

    fn ensure_index(&mut self, width: usize) {
        self.drain_external_changes();
        if self.indexed_width != Some(width) {
            for (component, entry) in self.inner.children.iter().zip(&mut self.entries) {
                let lines = component.borrow_mut().render(width);
                entry.last_height = lines.len();
                entry.cached = None;
                self.counters.height_updates += 1;
            }
            self.rebuild_heights();
            self.indexed_width = Some(width);
            self.dirty.clear();
            return;
        }
        let dirty = std::mem::take(&mut self.dirty);
        for index in dirty {
            let Some(component) = self.inner.children.get(index) else {
                continue;
            };
            let entry = &mut self.entries[index];
            let lines = component.borrow_mut().render(width);
            self.counters.height_updates += 1;
            self.heights.set(index, entry.last_height, lines.len());
            entry.last_height = lines.len();
            entry.cached = None;
        }
    }
}

impl Deref for TranscriptContainer {
    type Target = Container;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Component for TranscriptContainer {
    fn render(&mut self, width: usize) -> Vec<Line> {
        if !self.regular {
            self.ensure_index(width);
        }
        let mut lines = Vec::new();
        let first = if self.regular {
            self.first_regular_entry
        } else {
            0
        };
        for (relative_index, (component, entry)) in self.inner.children[first..]
            .iter()
            .zip(&mut self.entries[first..])
            .enumerate()
        {
            let rendered = if entry.stable {
                match &entry.cached {
                    Some((cached_width, cached)) if *cached_width == width => cached.clone(),
                    _ => {
                        let rendered = component.borrow_mut().render(width);
                        entry.cached = Some((width, rendered.clone()));
                        rendered
                    }
                }
            } else {
                component.borrow_mut().render(width)
            };
            if self.indexed_width == Some(width) {
                self.heights
                    .set(first + relative_index, entry.last_height, rendered.len());
                entry.last_height = rendered.len();
            }
            if self.regular {
                if !rendered.is_empty() {
                    self.active_candidates.insert(first + relative_index);
                } else {
                    self.active_candidates.remove(&(first + relative_index));
                }
            }
            if self.regular && first + relative_index == self.first_regular_entry {
                lines.extend(rendered.into_iter().skip(self.first_regular_offset));
            } else {
                lines.extend(rendered);
            }
        }
        lines
    }

    fn invalidate(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        for entry in &mut self.entries {
            entry.cached = None;
            entry.presentation_revision = entry.presentation_revision.wrapping_add(1);
        }
        self.inner.invalidate();
        self.indexed_width = None;
        self.dirty.clear();
        if self.stream_emitted.take().is_some() {
            // Invalidated components restart their stream from the top.
            self.history_reflow_needed = true;
        }
    }

    fn as_container(&self) -> Option<&Container> {
        Some(&self.inner)
    }

    fn windowed_content(
        &mut self,
        width: usize,
        first_row: usize,
        rows: usize,
    ) -> Option<WindowedContent> {
        self.ensure_index(width);
        let height = self.heights.total();
        if rows == 0 {
            return Some(WindowedContent {
                height,
                first_row,
                lines: Vec::new(),
            });
        }
        let end = first_row.saturating_add(rows).min(height);
        let mut lines = Vec::new();
        let mut visible = Vec::new();
        if first_row < end {
            let mut index = self.heights.entry_at(first_row);
            while index < self.entries.len() {
                let offset = self.heights.prefix(index);
                if offset >= end {
                    break;
                }
                let entry = &mut self.entries[index];
                self.counters.fullscreen_entries += 1;
                if entry.last_height > 0 {
                    let rendered = match &entry.cached {
                        Some((cached_width, cached)) if *cached_width == width => cached.clone(),
                        _ => {
                            let rendered = self.inner.children[index].borrow_mut().render(width);
                            entry.cached = Some((width, rendered.clone()));
                            rendered
                        }
                    };
                    let local_first = first_row.saturating_sub(offset);
                    lines.extend(
                        rendered
                            .into_iter()
                            .skip(local_first)
                            .take(end.min(offset + entry.last_height) - offset - local_first),
                    );
                    visible.push(index);
                }
                index += 1;
            }
        }
        if rows == 1 && self.window_cached.len() > 1 {
            for index in visible {
                if !self.window_cached.contains(&index)
                    && let Some(entry) = self.entries.get_mut(index)
                {
                    entry.cached = None;
                }
            }
            return Some(WindowedContent {
                height,
                first_row,
                lines,
            });
        }
        for old in std::mem::take(&mut self.window_cached) {
            if !visible.contains(&old)
                && let Some(entry) = self.entries.get_mut(old)
            {
                entry.cached = None;
            }
        }
        self.window_cached = visible;
        Some(WindowedContent {
            height,
            first_row,
            lines,
        })
    }

    fn content_revision(&self) -> u64 {
        self.revision
    }

    fn anchor_at(&mut self, width: usize, row: usize) -> Option<TranscriptAnchor> {
        self.ensure_index(width);
        if row >= self.heights.total() {
            return None;
        }
        let index = self.heights.entry_at(row);
        self.entries.get(index).map(|entry| TranscriptAnchor {
            entry_id: entry.id,
            row: row - self.heights.prefix(index),
        })
    }

    fn row_for_anchor(&mut self, width: usize, anchor: TranscriptAnchor) -> Option<usize> {
        self.ensure_index(width);
        let index = self
            .entries
            .binary_search_by_key(&anchor.entry_id, |entry| entry.id)
            .ok()?;
        Some(
            self.heights.prefix(index)
                + anchor
                    .row
                    .min(self.entries[index].last_height.saturating_sub(1)),
        )
    }

    fn prepare_reflow(&mut self, width: usize) {
        // The screen clears native scrollback on every reflow, so skipping
        // the replay here would drop the transcript from the terminal.
        if self.regular {
            for (component, entry) in self.inner.children.iter().zip(&self.entries) {
                if !entry.stable {
                    component.borrow_mut().prepare_reflow(width);
                }
            }
            self.replay_recent(width);
        }
    }

    fn history_regions(&mut self, width: usize, active_rows: usize) -> Option<HistoryRegions> {
        if !self.regular {
            return None;
        }
        self.drain_external_changes();
        if self.streamed_rows_diverged(width) {
            self.history_reflow_needed = true;
        }
        let rebuild = self.history_reflow_needed;
        if rebuild {
            self.replay_recent(width);
        }
        self.regular_width = Some(width);
        let mut history = Vec::new();
        while let Some(entry) = self.entries.get_mut(self.history_emitted_entry) {
            if !entry.stable {
                break;
            }
            let lines = match entry.cached.take() {
                Some((cached_width, lines)) if cached_width == width => lines,
                _ => self.inner.children[self.history_emitted_entry]
                    .borrow_mut()
                    .render(width),
            };
            self.counters.stable_entries += 1;
            let streamed = match self.stream_emitted.take() {
                Some(emitted) if emitted.id == entry.id => emitted.rows.len(),
                other => {
                    self.stream_emitted = other;
                    0
                }
            };
            if streamed > 0 {
                history.extend(lines.into_iter().skip(streamed));
            } else if self.history_emitted_entry == self.first_regular_entry {
                history.extend(lines.into_iter().skip(self.first_regular_offset));
            } else {
                history.extend(lines);
            }
            self.history_emitted_entry += 1;
        }
        if let Some(component) = self.inner.children.get(self.history_emitted_entry) {
            let streamed = component.borrow_mut().take_stream_history(width);
            if !streamed.is_empty() {
                // The committed rows leave the entry's render; a cached render
                // from before the commit would show them, and overflow them,
                // a second time.
                self.entries[self.history_emitted_entry].cached = None;
                let id = self.entries[self.history_emitted_entry].id;
                let covered = match &mut self.stream_emitted {
                    Some(emitted) if emitted.id == id => {
                        let covered = emitted.overflow.min(streamed.len());
                        emitted.overflow -= covered;
                        covered
                    }
                    _ => 0,
                };
                self.emit_stream_rows(id, &streamed[covered..], 0, &mut history);
            }
        }
        let mut remaining = active_rows;
        let mut chunks = Vec::new();
        let mut visible = Vec::new();
        let mut upper = self.entries.len();
        let stream_index = self.history_emitted_entry;
        let streams = self
            .inner
            .children
            .get(stream_index)
            .is_some_and(|component| component.borrow().streams_into_history());
        let mut stream_rendered = false;
        while remaining > 0 {
            let Some(index) = self
                .active_candidates
                .range(self.history_emitted_entry..upper)
                .next_back()
                .copied()
            else {
                break;
            };
            upper = index;
            let entry = &mut self.entries[index];
            self.counters.active_entries += 1;
            let lines = match &entry.cached {
                Some((cached_width, lines)) if *cached_width == width => lines.clone(),
                _ => {
                    let lines = self.inner.children[index].borrow_mut().render(width);
                    entry.cached = Some((width, lines.clone()));
                    lines
                }
            };
            if lines.is_empty() {
                self.active_candidates.remove(&index);
            }
            let mut lines = lines;
            if index == stream_index {
                // Rows cut from the top of a stream would be missing from
                // scrollback until it settles; they enter history as they
                // leave, and only the rest stays mutable. Rows already written
                // are skipped even once the entry stops qualifying.
                stream_rendered = true;
                let id = self.entries[index].id;
                let frozen = self.stream_overflow(id).min(lines.len());
                let leaving = if streams {
                    (lines.len() - frozen).saturating_sub(remaining)
                } else {
                    0
                };
                self.emit_stream_rows(id, &lines[frozen..frozen + leaving], leaving, &mut history);
                lines.drain(..frozen + leaving);
            }
            let first = lines.len().saturating_sub(remaining);
            remaining = remaining.saturating_sub(lines.len());
            chunks.push(lines.into_iter().skip(first).collect::<Vec<_>>());
            visible.push(index);
        }
        if streams
            && !stream_rendered
            && remaining == 0
            && self.active_candidates.contains(&stream_index)
        {
            // Entries below the stream fill the screen, so all of it is above.
            let id = self.entries[stream_index].id;
            let lines = self.inner.children[stream_index].borrow_mut().render(width);
            let frozen = self.stream_overflow(id).min(lines.len());
            self.emit_stream_rows(id, &lines[frozen..], lines.len() - frozen, &mut history);
        }
        for old in std::mem::take(&mut self.window_cached) {
            if !visible.contains(&old)
                && let Some(entry) = self.entries.get_mut(old)
            {
                entry.cached = None;
            }
        }
        self.window_cached = visible;
        let mut active = Vec::new();
        for chunk in chunks.into_iter().rev() {
            active.extend(chunk);
        }
        Some(HistoryRegions {
            history,
            active,
            rebuild,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::HeightIndex;

    #[test]
    fn height_index_locates_rows_after_early_height_changes() {
        let mut heights = vec![1, 3, 0, 2, 4, 1];
        let mut index = HeightIndex::from_heights(heights.iter().copied());
        for row in 0..index.total() {
            let expected = heights
                .iter()
                .scan(0, |offset, height| {
                    *offset += height;
                    Some(*offset)
                })
                .position(|end| end > row);
            assert_eq!(
                Some(index.entry_at(row)),
                expected,
                "wrong entry at row {row}"
            );
        }
        index.set(1, 3, 6);
        heights[1] = 6;
        index.push(5);
        heights.push(5);
        assert_eq!(index.total(), heights.iter().sum());
        assert_eq!(index.entry_at(4), 1);
        assert_eq!(index.entry_at(index.total() - 1), heights.len() - 1);
    }
}
