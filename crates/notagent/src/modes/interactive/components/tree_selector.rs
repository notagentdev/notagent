//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/tree-selector.ts` (1 437 LOC).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use notagent_ai::utils::js_number::{normalize_json_numbers, to_js_string};
use notagent_tui::components::input::Input;
use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::keybindings::{keybinding_keys, keybindings_match};
use notagent_tui::tui::{
    Component, ComponentRef, Container, Focusable, Line, component_ref, shared_lines,
};
use notagent_tui::utils::{
    slice_by_column, truncate_to_width, truncate_to_width_opts, visible_width, wrap_text_with_ansi,
};
use regex::Regex;
use serde_json::{Map, Value};

use crate::core::session_manager::{SessionEntry, SessionTreeNode};
use crate::modes::interactive::theme::theme::{ThemeBg, ThemeColor, theme};

use super::dynamic_border::DynamicBorder;
use super::keybinding_hints::{KeyTextFormatOptions, format_key_text, key_hint};

/// Gutter info: position (displayIndent where connector was) and whether to show │
#[derive(Clone, Debug)]
struct GutterInfo {
    /// displayIndent level where the connector was shown
    position: usize,
    /// true = show │, false = show spaces
    show: bool,
}

/// The node payload a [`FlatNode`] carries.
///
/// The TypeScript `FlatNode.node` is a reference to the caller's
/// `SessionTreeNode`; `children` is never read through it, so the port keeps
/// only the fields the component uses (and `updateNodeLabel` writes).
#[derive(Clone, Debug)]
pub struct TreeNodeData {
    /// The session entry.
    pub entry: SessionEntry,
    /// Resolved label for this entry, if any.
    pub label: Option<String>,
    /// Timestamp of the latest label change for this entry, if any.
    pub label_timestamp: Option<String>,
}

/// Flattened tree node for navigation
#[derive(Clone, Debug)]
struct FlatNode {
    node: TreeNodeData,
    /// Indentation level (each level = 3 chars)
    indent: usize,
    /// Whether to show connector (├─ or └─) - true if parent has multiple children
    show_connector: bool,
    /// If showConnector, true = last sibling (└─), false = not last (├─)
    is_last: bool,
    /// Gutter info for each ancestor branch point
    gutters: Vec<GutterInfo>,
    /// True if this node is a root under a virtual branching root (multiple roots)
    is_virtual_root_child: bool,
}

struct HorizontalViewportRow {
    gutter: String,
    body: String,
    anchor_col: usize,
    body_width: usize,
    is_selected: bool,
}

const TREE_GUTTER_WIDTH: usize = 2;
const MIN_VISIBLE_ANCHOR_CONTENT_WIDTH: usize = 4;
const MAX_VISIBLE_ANCHOR_CONTENT_WIDTH: usize = 20;
const MIN_ANCHOR_CONTEXT_WIDTH: usize = 2;
const MAX_ANCHOR_CONTEXT_WIDTH: usize = 12;

/// Render tree rows into a horizontally clipped viewport.
///
/// The tree gutter is always kept visible. The row bodies are shifted left only
/// when the selected row's anchor (the start of its entry text after tree
/// indentation/markers) would otherwise be too far right to see useful content.
fn render_horizontal_viewport(rows: &[HorizontalViewportRow], width: usize) -> Vec<String> {
    let viewport_width = width.saturating_sub(TREE_GUTTER_WIDTH);
    let max_body_width = rows.iter().fold(0, |max, row| max.max(row.body_width));
    let max_horizontal_scroll = max_body_width.saturating_sub(viewport_width);
    let selected_row = rows.iter().find(|row| row.is_selected);

    // Only pan horizontally when needed to keep enough selected-row content visible after its anchor.
    let mut horizontal_scroll = 0usize;
    if let Some(selected_row) = selected_row
        && max_horizontal_scroll > 0
    {
        let min_visible_anchor_content_width = MAX_VISIBLE_ANCHOR_CONTENT_WIDTH
            .min(MIN_VISIBLE_ANCHOR_CONTENT_WIDTH.max(viewport_width / 3));
        if selected_row.anchor_col > viewport_width.saturating_sub(min_visible_anchor_content_width)
        {
            let anchor_context_width =
                MAX_ANCHOR_CONTEXT_WIDTH.min(MIN_ANCHOR_CONTEXT_WIDTH.max(viewport_width / 4));
            horizontal_scroll =
                max_horizontal_scroll.min(selected_row.anchor_col - anchor_context_width);
        }
    }

    // Clip only the body; the fixed-width gutter remains visible as navigation context.
    rows.iter()
        .map(|row| {
            let line = if horizontal_scroll > 0 {
                format!(
                    "{}{}\x1b[0m",
                    row.gutter,
                    slice_by_column(&row.body, horizontal_scroll, viewport_width, true)
                )
            } else {
                format!("{}{}", row.gutter, row.body)
            };
            truncate_to_width_opts(&line, width, "", false)
        })
        .collect()
}

/// Filter mode for tree display
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FilterMode {
    /// Hides settings/bookkeeping entries.
    #[default]
    Default,
    /// Default minus tool results.
    NoTools,
    /// Only user messages.
    UserOnly,
    /// Only labelled entries.
    LabeledOnly,
    /// Everything.
    All,
}

const FILTER_MODES: [FilterMode; 5] = [
    FilterMode::Default,
    FilterMode::NoTools,
    FilterMode::UserOnly,
    FilterMode::LabeledOnly,
    FilterMode::All,
];

/// Tool call info for lookup
#[derive(Clone, Debug)]
struct ToolCallInfo {
    name: String,
    arguments: Map<String, Value>,
}

/// Invoked with the entry id the user confirmed.
pub type TreeSelectCallback = Box<dyn FnMut(&str)>;
/// Invoked with the label editor's result.
pub type LabelChangeCallback = Box<dyn FnMut(&str, Option<&str>)>;
/// Invoked with the text of the copied entry.
pub type CopyCallback = Box<dyn FnMut(Option<&str>)>;

// --- JavaScript string helpers -----------------------------------------------

/// `String.prototype.slice(0, limit)` counts UTF-16 code units. A cut inside a
/// surrogate pair would produce a lone surrogate, which Rust strings cannot
/// hold, so the port stops before that character.
fn js_slice(text: &str, limit: usize) -> &str {
    let mut units = 0usize;
    for (offset, character) in text.char_indices() {
        let next = units + character.len_utf16();
        if next > limit {
            return &text[..offset];
        }
        units = next;
    }
    text
}

/// `String.prototype.length` in UTF-16 code units.
fn js_length(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// `s.replace(/[\n\t]/g, " ").trim()`
fn normalize(text: &str) -> String {
    text.replace(['\n', '\t'], " ").trim().to_string()
}

/// `String(value)` for a JSON value used as a tool argument.
fn js_string(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.as_f64().map(to_js_string).unwrap_or_default(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(other) => other.to_string(),
    }
}

/// `JSON.stringify(args)` with JavaScript number formatting.
fn js_stringify(arguments: &Map<String, Value>) -> String {
    let mut value = Value::Object(arguments.clone());
    normalize_json_numbers(&mut value);
    serde_json::to_string(&value).unwrap_or_default()
}

fn message_field<'a>(message: &'a Value, key: &str) -> Option<&'a Value> {
    message.get(key)
}

fn message_role(message: &Value) -> &str {
    message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// Tree list component with selection and ASCII art visualization
pub struct TreeList {
    flat_nodes: Vec<FlatNode>,
    /// Indices into `flat_nodes`; the TypeScript version holds references to the
    /// same objects, so `recalculateVisualStructure` writes through both views.
    filtered_nodes: Vec<usize>,
    /// `number` in TypeScript: navigation on an empty list produces -1.
    selected_index: isize,
    current_leaf_id: Option<String>,
    max_visible_lines: usize,
    filter_mode: FilterMode,
    search_query: String,
    tool_call_map: HashMap<String, ToolCallInfo>,
    multiple_roots: bool,
    show_label_timestamps: bool,
    active_path_ids: HashSet<String>,
    visible_parent_map: HashMap<String, Option<String>>,
    visible_children_map: HashMap<Option<String>, Vec<String>>,
    last_selected_id: Option<String>,
    folded_nodes: HashSet<String>,

    /// Invoked with the confirmed entry id.
    pub on_select: Option<TreeSelectCallback>,
    /// Invoked when the selector is cancelled.
    pub on_cancel: Option<Box<dyn FnMut()>>,
    /// `onCopy`/`onLabelEdit` call back into the owning selector in TypeScript;
    /// the port records the request and the selector drains it after dispatch.
    pending_copy: Option<Option<String>>,
    pending_label_edit: Option<(String, Option<String>)>,
}

impl TreeList {
    fn new(
        tree: &[SessionTreeNode],
        current_leaf_id: Option<&str>,
        max_visible_lines: usize,
        initial_selected_id: Option<&str>,
        initial_filter_mode: Option<FilterMode>,
    ) -> Self {
        let mut list = Self {
            flat_nodes: Vec::new(),
            filtered_nodes: Vec::new(),
            selected_index: 0,
            current_leaf_id: current_leaf_id.map(str::to_string),
            max_visible_lines,
            filter_mode: initial_filter_mode.unwrap_or_default(),
            search_query: String::new(),
            tool_call_map: HashMap::new(),
            multiple_roots: tree.len() > 1,
            show_label_timestamps: false,
            active_path_ids: HashSet::new(),
            visible_parent_map: HashMap::new(),
            visible_children_map: HashMap::new(),
            last_selected_id: None,
            folded_nodes: HashSet::new(),
            on_select: None,
            on_cancel: None,
            pending_copy: None,
            pending_label_edit: None,
        };
        list.flat_nodes = list.flatten_tree(tree);
        list.build_active_path();
        list.apply_filter();

        // Start with initialSelectedId if provided, otherwise current leaf
        let target_id = initial_selected_id
            .map(str::to_string)
            .or_else(|| list.current_leaf_id.clone());
        list.selected_index = list.find_nearest_visible_index(target_id.as_deref());
        list.last_selected_id = list.selected_entry_id();
        list
    }

    fn flat(&self, index: isize) -> Option<&FlatNode> {
        if index < 0 {
            return None;
        }
        self.filtered_nodes
            .get(index as usize)
            .map(|position| &self.flat_nodes[*position])
    }

    fn selected_entry_id(&self) -> Option<String> {
        self.flat(self.selected_index)
            .map(|node| node.node.entry.id().to_string())
    }

    /// Find the index of the nearest visible entry, walking up the parent chain if needed.
    /// Returns the index in filteredNodes, or the last index as fallback.
    fn find_nearest_visible_index(&self, entry_id: Option<&str>) -> isize {
        if self.filtered_nodes.is_empty() {
            return 0;
        }

        // Build a map for parent lookup
        let entry_map: HashMap<&str, &FlatNode> = self
            .flat_nodes
            .iter()
            .map(|node| (node.node.entry.id(), node))
            .collect();

        // Build a map of visible entry IDs to their indices in filteredNodes
        let visible_id_to_index: HashMap<&str, usize> = self
            .filtered_nodes
            .iter()
            .enumerate()
            .map(|(index, position)| (self.flat_nodes[*position].node.entry.id(), index))
            .collect();

        // Walk from entryId up to root, looking for a visible entry
        let mut current_id = entry_id;
        while let Some(id) = current_id {
            if let Some(index) = visible_id_to_index.get(id) {
                return *index as isize;
            }
            let Some(node) = entry_map.get(id) else {
                break;
            };
            current_id = node.node.entry.parent_id();
        }

        // Fallback: last visible entry
        self.filtered_nodes.len() as isize - 1
    }

    /// Build the set of entry IDs on the path from root to current leaf
    fn build_active_path(&mut self) {
        self.active_path_ids.clear();
        let Some(leaf_id) = self.current_leaf_id.clone() else {
            return;
        };

        // Build a map of id -> entry for parent lookup
        let entry_map: HashMap<&str, &FlatNode> = self
            .flat_nodes
            .iter()
            .map(|node| (node.node.entry.id(), node))
            .collect();

        // Walk from leaf to root
        let mut active: HashSet<String> = HashSet::new();
        let mut current_id: Option<&str> = Some(leaf_id.as_str());
        while let Some(id) = current_id {
            active.insert(id.to_string());
            let Some(node) = entry_map.get(id) else {
                break;
            };
            current_id = node.node.entry.parent_id();
        }
        self.active_path_ids = active;
    }

    fn flatten_tree(&mut self, roots: &[SessionTreeNode]) -> Vec<FlatNode> {
        let mut result: Vec<FlatNode> = Vec::new();
        self.tool_call_map.clear();

        // Indentation rules:
        // - At indent 0: stay at 0 unless parent has >1 children (then +1)
        // - At indent 1: children always go to indent 2 (visual grouping of subtree)
        // - At indent 2+: stay flat for single-child chains, +1 only if parent branches

        // Stack items: [node, indent, justBranched, showConnector, isLast, gutters, isVirtualRootChild]
        struct StackItem<'a> {
            node: &'a SessionTreeNode,
            indent: usize,
            just_branched: bool,
            show_connector: bool,
            is_last: bool,
            gutters: Vec<GutterInfo>,
            is_virtual_root_child: bool,
        }
        let mut stack: Vec<StackItem> = Vec::new();

        // Determine which subtrees contain the active leaf (to sort current branch first)
        // Use iterative post-order traversal to avoid stack overflow
        let mut contains_active: HashMap<*const SessionTreeNode, bool> = HashMap::new();
        let leaf_id = self.current_leaf_id.clone();
        {
            // Build list in pre-order, then process in reverse for post-order effect
            let mut all_nodes: Vec<&SessionTreeNode> = Vec::new();
            let mut pre_order_stack: Vec<&SessionTreeNode> = roots.iter().collect();
            while let Some(node) = pre_order_stack.pop() {
                all_nodes.push(node);
                // Push children in reverse so they're processed left-to-right
                for child in node.children.iter().rev() {
                    pre_order_stack.push(child);
                }
            }
            // Process in reverse (post-order): children before parents
            for node in all_nodes.iter().rev() {
                let mut has = leaf_id
                    .as_deref()
                    .is_some_and(|leaf| node.entry.id() == leaf);
                for child in &node.children {
                    if contains_active
                        .get(&(child as *const SessionTreeNode))
                        .copied()
                        .unwrap_or(false)
                    {
                        has = true;
                    }
                }
                contains_active.insert(*node as *const SessionTreeNode, has);
            }
        }
        let active = |node: &SessionTreeNode| -> bool {
            contains_active
                .get(&(node as *const SessionTreeNode))
                .copied()
                .unwrap_or(false)
        };

        // Add roots in reverse order, prioritizing the one containing the active leaf
        // If multiple roots, treat them as children of a virtual root that branches
        let multiple_roots = roots.len() > 1;
        let mut ordered_roots: Vec<&SessionTreeNode> = roots.iter().collect();
        // `sort((a, b) => Number(active(b)) - Number(active(a)))` — stable.
        ordered_roots.sort_by_key(|node| !active(node));
        for (index, root) in ordered_roots.iter().enumerate().rev() {
            let is_last = index == ordered_roots.len() - 1;
            stack.push(StackItem {
                node: root,
                indent: usize::from(multiple_roots),
                just_branched: multiple_roots,
                show_connector: multiple_roots,
                is_last,
                gutters: Vec::new(),
                is_virtual_root_child: multiple_roots,
            });
        }

        while let Some(item) = stack.pop() {
            // Extract tool calls from assistant messages for later lookup
            let entry = &item.node.entry;
            if let Some(message_entry) = entry.as_message()
                && message_role(&message_entry.message) == "assistant"
                && let Some(Value::Array(content)) =
                    message_field(&message_entry.message, "content")
            {
                for block in content {
                    if block.get("type").and_then(Value::as_str) == Some("toolCall") {
                        let id = block.get("id").and_then(Value::as_str).unwrap_or_default();
                        let name = block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let arguments = match block.get("arguments") {
                            Some(Value::Object(map)) => map.clone(),
                            _ => Map::new(),
                        };
                        self.tool_call_map.insert(
                            id.to_string(),
                            ToolCallInfo {
                                name: name.to_string(),
                                arguments,
                            },
                        );
                    }
                }
            }

            result.push(FlatNode {
                node: TreeNodeData {
                    entry: item.node.entry.clone(),
                    label: item.node.label.clone(),
                    label_timestamp: item.node.label_timestamp.clone(),
                },
                indent: item.indent,
                show_connector: item.show_connector,
                is_last: item.is_last,
                gutters: item.gutters.clone(),
                is_virtual_root_child: item.is_virtual_root_child,
            });

            let children = &item.node.children;
            let multiple_children = children.len() > 1;

            // Order children so the branch containing the active leaf comes first
            let ordered_children: Vec<&SessionTreeNode> = {
                let mut prioritized: Vec<&SessionTreeNode> = Vec::new();
                let mut rest: Vec<&SessionTreeNode> = Vec::new();
                for child in children {
                    if active(child) {
                        prioritized.push(child);
                    } else {
                        rest.push(child);
                    }
                }
                prioritized.extend(rest);
                prioritized
            };

            // Calculate child indent
            let child_indent = if multiple_children {
                // Parent branches: children get +1
                item.indent + 1
            } else if item.just_branched && item.indent > 0 {
                // First generation after a branch: +1 for visual grouping
                item.indent + 1
            } else {
                // Single-child chain: stay flat
                item.indent
            };

            // Build gutters for children
            // If this node showed a connector, add a gutter entry for descendants
            // Only add gutter if connector is actually displayed (not suppressed for virtual root children)
            let connector_displayed = item.show_connector && !item.is_virtual_root_child;
            // When connector is displayed, add a gutter entry at the connector's position
            // Connector is at position (displayIndent - 1), so gutter should be there too
            let current_display_indent = if self.multiple_roots {
                item.indent.saturating_sub(1)
            } else {
                item.indent
            };
            let connector_position = current_display_indent.saturating_sub(1);
            let child_gutters: Vec<GutterInfo> = if connector_displayed {
                let mut gutters = item.gutters.clone();
                gutters.push(GutterInfo {
                    position: connector_position,
                    show: !item.is_last,
                });
                gutters
            } else {
                item.gutters.clone()
            };

            // Add children in reverse order
            for (index, child) in ordered_children.iter().enumerate().rev() {
                let child_is_last = index == ordered_children.len() - 1;
                stack.push(StackItem {
                    node: child,
                    indent: child_indent,
                    just_branched: multiple_children,
                    show_connector: multiple_children,
                    is_last: child_is_last,
                    gutters: child_gutters.clone(),
                    is_virtual_root_child: false,
                });
            }
        }

        result
    }

    fn apply_filter(&mut self) {
        // Update lastSelectedId only when we have a valid selection (non-empty list)
        // This preserves the selection when switching through empty filter results
        if !self.filtered_nodes.is_empty()
            && let Some(id) = self.selected_entry_id()
        {
            self.last_selected_id = Some(id);
        }

        let lowered = self.search_query.to_lowercase();
        let search_tokens: Vec<&str> = lowered.split_whitespace().collect();

        let current_leaf_id = self.current_leaf_id.clone();
        let filter_mode = self.filter_mode;
        let mut filtered: Vec<usize> = Vec::new();
        for (position, flat_node) in self.flat_nodes.iter().enumerate() {
            let entry = &flat_node.node.entry;
            let is_current_leaf = Some(entry.id()) == current_leaf_id.as_deref();

            // Skip assistant messages with only tool calls (no text) unless error/aborted
            // Always show current leaf so active position is visible
            if let Some(message_entry) = entry.as_message()
                && message_role(&message_entry.message) == "assistant"
                && !is_current_leaf
            {
                let message = &message_entry.message;
                let has_text = has_text_content(message_field(message, "content"));
                let stop_reason = message
                    .get("stopReason")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let is_error_or_aborted =
                    !stop_reason.is_empty() && stop_reason != "stop" && stop_reason != "toolUse";
                // Only hide if no text AND not an error/aborted message
                if !has_text && !is_error_or_aborted {
                    continue;
                }
            }

            // Apply filter mode
            // Entry types hidden in default view (settings/bookkeeping)
            let is_settings_entry = matches!(
                entry,
                SessionEntry::Label(_)
                    | SessionEntry::Custom(_)
                    | SessionEntry::ModelChange(_)
                    | SessionEntry::ThinkingLevelChange(_)
                    | SessionEntry::SessionInfo(_)
            );

            let passes_filter = match filter_mode {
                // Just user messages
                FilterMode::UserOnly => entry
                    .as_message()
                    .is_some_and(|message| message_role(&message.message) == "user"),
                // Default minus tool results
                // `!isSettingsEntry && !(type === "message" && role === "toolResult")`
                FilterMode::NoTools => {
                    !is_settings_entry
                        && entry
                            .as_message()
                            .is_none_or(|message| message_role(&message.message) != "toolResult")
                }
                // Just labeled entries
                FilterMode::LabeledOnly => flat_node.node.label.is_some(),
                // Show everything
                FilterMode::All => true,
                // Default mode: hide settings/bookkeeping entries
                FilterMode::Default => !is_settings_entry,
            };

            if !passes_filter {
                continue;
            }

            // Apply search filter
            if !search_tokens.is_empty() {
                let node_text = self.get_searchable_text(&flat_node.node).to_lowercase();
                if !search_tokens.iter().all(|token| node_text.contains(token)) {
                    continue;
                }
            }

            filtered.push(position);
        }
        self.filtered_nodes = filtered;

        // Filter out descendants of folded nodes.
        if !self.folded_nodes.is_empty() {
            let mut skip_set: HashSet<&str> = HashSet::new();
            for flat_node in &self.flat_nodes {
                let id = flat_node.node.entry.id();
                if let Some(parent_id) = flat_node.node.entry.parent_id()
                    && (self.folded_nodes.contains(parent_id) || skip_set.contains(parent_id))
                {
                    skip_set.insert(id);
                }
            }
            let skipped: HashSet<String> = skip_set.into_iter().map(str::to_string).collect();
            self.filtered_nodes
                .retain(|position| !skipped.contains(self.flat_nodes[*position].node.entry.id()));
        }

        // Recalculate visual structure (indent, connectors, gutters) based on visible tree
        self.recalculate_visual_structure();

        // Try to preserve cursor on the same node, or find nearest visible ancestor
        if let Some(last_selected_id) = self.last_selected_id.clone() {
            self.selected_index = self.find_nearest_visible_index(Some(&last_selected_id));
        } else if self.selected_index >= self.filtered_nodes.len() as isize {
            // Clamp index if out of bounds
            self.selected_index = (self.filtered_nodes.len() as isize - 1).max(0);
        }

        // Update lastSelectedId to the actual selection (may have changed due to parent walk)
        if !self.filtered_nodes.is_empty()
            && let Some(id) = self.selected_entry_id()
        {
            self.last_selected_id = Some(id);
        }
    }

    /// Recompute indentation/connectors for the filtered view
    ///
    /// Filtering can hide intermediate entries; descendants attach to the nearest visible ancestor.
    /// Keep indentation semantics aligned with flattenTree() so single-child chains don't drift right.
    fn recalculate_visual_structure(&mut self) {
        if self.filtered_nodes.is_empty() {
            return;
        }

        let visible_ids: HashSet<String> = self
            .filtered_nodes
            .iter()
            .map(|position| self.flat_nodes[*position].node.entry.id().to_string())
            .collect();

        // Build entry map for efficient parent lookup (using full tree)
        let parent_of: HashMap<String, Option<String>> = self
            .flat_nodes
            .iter()
            .map(|node| {
                (
                    node.node.entry.id().to_string(),
                    node.node.entry.parent_id().map(str::to_string),
                )
            })
            .collect();

        // Find nearest visible ancestor for a node
        let find_visible_ancestor = |node_id: &str| -> Option<String> {
            let mut current_id = parent_of.get(node_id).cloned().flatten();
            while let Some(id) = current_id {
                if visible_ids.contains(&id) {
                    return Some(id);
                }
                current_id = parent_of.get(&id).cloned().flatten();
            }
            None
        };

        // Build visible tree structure:
        // - visibleParent: nodeId → nearest visible ancestor (or null for roots)
        // - visibleChildren: parentId → list of visible children (in filteredNodes order)
        let mut visible_parent: HashMap<String, Option<String>> = HashMap::new();
        let mut visible_children: HashMap<Option<String>, Vec<String>> = HashMap::new();
        visible_children.insert(None, Vec::new()); // root-level nodes

        for position in &self.filtered_nodes {
            let node_id = self.flat_nodes[*position].node.entry.id().to_string();
            let ancestor_id = find_visible_ancestor(&node_id);
            visible_parent.insert(node_id.clone(), ancestor_id.clone());
            visible_children
                .entry(ancestor_id)
                .or_default()
                .push(node_id);
        }

        // Update multipleRoots based on visible roots
        let visible_root_ids = visible_children.get(&None).cloned().unwrap_or_default();
        self.multiple_roots = visible_root_ids.len() > 1;

        // Build a map for quick lookup: nodeId → FlatNode
        let filtered_node_map: HashMap<String, usize> = self
            .filtered_nodes
            .iter()
            .map(|position| {
                (
                    self.flat_nodes[*position].node.entry.id().to_string(),
                    *position,
                )
            })
            .collect();

        // DFS over the visible tree using flattenTree() indentation semantics
        // Stack items: [nodeId, indent, justBranched, showConnector, isLast, gutters, isVirtualRootChild]
        struct StackItem {
            node_id: String,
            indent: usize,
            just_branched: bool,
            show_connector: bool,
            is_last: bool,
            gutters: Vec<GutterInfo>,
            is_virtual_root_child: bool,
        }
        let mut stack: Vec<StackItem> = Vec::new();

        // Add visible roots in reverse order (to process in forward order via stack)
        for (index, root_id) in visible_root_ids.iter().enumerate().rev() {
            let is_last = index == visible_root_ids.len() - 1;
            stack.push(StackItem {
                node_id: root_id.clone(),
                indent: usize::from(self.multiple_roots),
                just_branched: self.multiple_roots,
                show_connector: self.multiple_roots,
                is_last,
                gutters: Vec::new(),
                is_virtual_root_child: self.multiple_roots,
            });
        }

        while let Some(item) = stack.pop() {
            let Some(position) = filtered_node_map.get(&item.node_id).copied() else {
                continue;
            };

            // Update this node's visual properties
            {
                let flat_node = &mut self.flat_nodes[position];
                flat_node.indent = item.indent;
                flat_node.show_connector = item.show_connector;
                flat_node.is_last = item.is_last;
                flat_node.gutters = item.gutters.clone();
                flat_node.is_virtual_root_child = item.is_virtual_root_child;
            }

            // Get visible children of this node
            let children = visible_children
                .get(&Some(item.node_id.clone()))
                .cloned()
                .unwrap_or_default();
            let multiple_children = children.len() > 1;

            // Child indent follows flattenTree(): branch points (and first
            // generation after a branch) shift +1, single-child chains stay flat.
            let child_indent = if multiple_children || (item.just_branched && item.indent > 0) {
                item.indent + 1
            } else {
                item.indent
            };

            // Child gutters follow flattenTree() connector/gutter rules
            let connector_displayed = item.show_connector && !item.is_virtual_root_child;
            let current_display_indent = if self.multiple_roots {
                item.indent.saturating_sub(1)
            } else {
                item.indent
            };
            let connector_position = current_display_indent.saturating_sub(1);
            let child_gutters: Vec<GutterInfo> = if connector_displayed {
                let mut gutters = item.gutters.clone();
                gutters.push(GutterInfo {
                    position: connector_position,
                    show: !item.is_last,
                });
                gutters
            } else {
                item.gutters.clone()
            };

            // Add children in reverse order (to process in forward order via stack)
            for (index, child_id) in children.iter().enumerate().rev() {
                let child_is_last = index == children.len() - 1;
                stack.push(StackItem {
                    node_id: child_id.clone(),
                    indent: child_indent,
                    just_branched: multiple_children,
                    show_connector: multiple_children,
                    is_last: child_is_last,
                    gutters: child_gutters.clone(),
                    is_virtual_root_child: false,
                });
            }
        }

        // Store visible tree maps for ancestor/descendant lookups in navigation
        self.visible_parent_map = visible_parent;
        self.visible_children_map = visible_children;
    }

    /// Get searchable text content from a node
    fn get_searchable_text(&self, node: &TreeNodeData) -> String {
        let entry = &node.entry;
        let mut parts: Vec<String> = Vec::new();

        if let Some(label) = node.label.as_ref() {
            parts.push(label.clone());
        }

        match entry {
            SessionEntry::Message(message_entry) => {
                let message = &message_entry.message;
                let role = message_role(message);
                parts.push(role.to_string());
                if let Some(content) = message_field(message, "content")
                    && !content.is_null()
                    && !matches!(content, Value::String(text) if text.is_empty())
                {
                    parts.push(extract_content(Some(content)));
                }
                if role == "bashExecution"
                    && let Some(command) = message.get("command").and_then(Value::as_str)
                    && !command.is_empty()
                {
                    parts.push(command.to_string());
                }
            }
            SessionEntry::CustomMessage(custom) => {
                parts.push(custom.custom_type.clone());
                if let Value::String(content) = &custom.content {
                    parts.push(content.clone());
                } else {
                    parts.push(extract_content(Some(&custom.content)));
                }
            }
            SessionEntry::Compaction(_) => parts.push("compaction".to_string()),
            SessionEntry::BranchSummary(summary) => {
                parts.push("branch summary".to_string());
                parts.push(summary.summary.clone());
            }
            SessionEntry::SessionInfo(info) => {
                parts.push("title".to_string());
                if let Some(name) = info.name.as_ref()
                    && !name.is_empty()
                {
                    parts.push(name.clone());
                }
            }
            SessionEntry::ModelChange(change) => {
                parts.push("model".to_string());
                parts.push(change.model_id.clone());
            }
            SessionEntry::ThinkingLevelChange(change) => {
                parts.push("thinking".to_string());
                parts.push(change.thinking_level.clone());
            }
            SessionEntry::Custom(custom) => {
                parts.push("custom".to_string());
                parts.push(custom.custom_type.clone());
            }
            SessionEntry::Label(label) => {
                parts.push("label".to_string());
                parts.push(label.label.clone().unwrap_or_default());
            }
            SessionEntry::Unknown(_) => {}
        }

        parts.join(" ")
    }

    /// The current search query.
    pub fn get_search_query(&self) -> &str {
        &self.search_query
    }

    /// The node under the cursor.
    pub fn get_selected_node(&self) -> Option<&TreeNodeData> {
        self.flat(self.selected_index).map(|node| &node.node)
    }

    /// Queue the selected entry's text for the clipboard.
    pub fn copy_selected(&mut self) {
        let text = self.get_selected_node().map(get_entry_copy_text);
        self.pending_copy = Some(text.unwrap_or(None));
    }

    /// Set or clear a node's label.
    pub fn update_node_label(
        &mut self,
        entry_id: &str,
        label: Option<&str>,
        label_timestamp: Option<&str>,
    ) {
        for flat_node in &mut self.flat_nodes {
            if flat_node.node.entry.id() == entry_id {
                flat_node.node.label = label.map(str::to_string);
                flat_node.node.label_timestamp = label.map(|_| {
                    label_timestamp
                        .map(str::to_string)
                        .unwrap_or_else(now_iso_string)
                });
                break;
            }
        }
    }

    fn get_status_labels(&self) -> String {
        let mut labels = String::new();
        match self.filter_mode {
            FilterMode::NoTools => labels.push_str(" [no-tools]"),
            FilterMode::UserOnly => labels.push_str(" [user]"),
            FilterMode::LabeledOnly => labels.push_str(" [labeled]"),
            FilterMode::All => labels.push_str(" [all]"),
            FilterMode::Default => {}
        }
        if self.show_label_timestamps {
            labels.push_str(" [+label time]");
        }
        labels
    }
}

impl TreeList {
    fn get_entry_display_text(&self, node: &TreeNodeData, is_selected: bool) -> String {
        let entry = &node.entry;
        let theme_instance = theme();

        let result = match entry {
            SessionEntry::Message(message_entry) => {
                let message = &message_entry.message;
                let role = message_role(message);
                if role == "user" {
                    let content = normalize(&extract_content(message_field(message, "content")));
                    theme_instance.fg(ThemeColor::Accent, "user: ") + &content
                } else if role == "assistant" {
                    let text_content =
                        normalize(&extract_content(message_field(message, "content")));
                    let stop_reason = message.get("stopReason").and_then(Value::as_str);
                    let error_message = message
                        .get("errorMessage")
                        .and_then(Value::as_str)
                        .filter(|text| !text.is_empty());
                    if !text_content.is_empty() {
                        theme_instance.fg(ThemeColor::Success, "assistant: ") + &text_content
                    } else if stop_reason == Some("aborted") {
                        theme_instance.fg(ThemeColor::Success, "assistant: ")
                            + &theme_instance.fg(ThemeColor::Muted, "(aborted)")
                    } else if let Some(error_message) = error_message {
                        let error_message = js_slice(&normalize(error_message), 80).to_string();
                        theme_instance.fg(ThemeColor::Success, "assistant: ")
                            + &theme_instance.fg(ThemeColor::Error, &error_message)
                    } else {
                        theme_instance.fg(ThemeColor::Success, "assistant: ")
                            + &theme_instance.fg(ThemeColor::Muted, "(no content)")
                    }
                } else if role == "toolResult" {
                    let tool_call = message
                        .get("toolCallId")
                        .and_then(Value::as_str)
                        .and_then(|id| self.tool_call_map.get(id));
                    if let Some(tool_call) = tool_call {
                        theme_instance.fg(
                            ThemeColor::Muted,
                            &format_tool_call(&tool_call.name, &tool_call.arguments),
                        )
                    } else {
                        let tool_name = message
                            .get("toolName")
                            .and_then(Value::as_str)
                            .filter(|name| !name.is_empty())
                            .unwrap_or("tool");
                        theme_instance.fg(ThemeColor::Muted, &format!("[{tool_name}]"))
                    }
                } else if role == "bashExecution" {
                    let command = message.get("command").and_then(Value::as_str).unwrap_or("");
                    theme_instance.fg(ThemeColor::Dim, &format!("[bash]: {}", normalize(command)))
                } else {
                    theme_instance.fg(ThemeColor::Dim, &format!("[{role}]"))
                }
            }
            SessionEntry::CustomMessage(custom) => {
                let content = match &custom.content {
                    Value::String(text) => text.clone(),
                    Value::Array(blocks) => blocks
                        .iter()
                        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                        .map(|block| {
                            block
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string()
                        })
                        .collect::<Vec<_>>()
                        .join(""),
                    _ => String::new(),
                };
                theme_instance.fg(
                    ThemeColor::CustomMessageLabel,
                    &format!("[{}]: ", custom.custom_type),
                ) + &normalize(&content)
            }
            SessionEntry::Compaction(compaction) => {
                // `Math.round(tokensBefore / 1000)` — JavaScript rounds halves up.
                let tokens = ((compaction.tokens_before as f64) / 1000.0 + 0.5).floor();
                theme_instance.fg(
                    ThemeColor::BorderAccent,
                    &format!("[compaction: {}k tokens]", to_js_string(tokens)),
                )
            }
            SessionEntry::BranchSummary(summary) => {
                theme_instance.fg(ThemeColor::Warning, "[branch summary]: ")
                    + &normalize(&summary.summary)
            }
            SessionEntry::ModelChange(change) => {
                theme_instance.fg(ThemeColor::Dim, &format!("[model: {}]", change.model_id))
            }
            SessionEntry::ThinkingLevelChange(change) => theme_instance.fg(
                ThemeColor::Dim,
                &format!("[thinking: {}]", change.thinking_level),
            ),
            SessionEntry::Custom(custom) => theme_instance.fg(
                ThemeColor::Dim,
                &format!("[custom: {}]", custom.custom_type),
            ),
            SessionEntry::Label(label) => theme_instance.fg(
                ThemeColor::Dim,
                &format!("[label: {}]", label.label.as_deref().unwrap_or("(cleared)")),
            ),
            SessionEntry::SessionInfo(info) => match info.name.as_deref() {
                Some(name) if !name.is_empty() => {
                    theme_instance.fg(ThemeColor::Dim, "[title: ")
                        + &theme_instance.fg(ThemeColor::Dim, name)
                        + &theme_instance.fg(ThemeColor::Dim, "]")
                }
                _ => {
                    theme_instance.fg(ThemeColor::Dim, "[title: ")
                        + &theme_instance.italic(&theme_instance.fg(ThemeColor::Dim, "empty"))
                        + &theme_instance.fg(ThemeColor::Dim, "]")
                }
            },
            SessionEntry::Unknown(_) => String::new(),
        };

        if is_selected {
            theme_instance.bold(&result)
        } else {
            result
        }
    }

    /// Whether a node can be folded. A node is foldable if it has visible children
    /// and is either a root (no visible parent) or a segment start (visible parent
    /// has multiple visible children).
    fn is_foldable(&self, entry_id: &str) -> bool {
        let Some(children) = self.visible_children_map.get(&Some(entry_id.to_string())) else {
            return false;
        };
        if children.is_empty() {
            return false;
        }
        let Some(Some(parent_id)) = self.visible_parent_map.get(entry_id) else {
            return true;
        };
        self.visible_children_map
            .get(&Some(parent_id.clone()))
            .is_some_and(|siblings| siblings.len() > 1)
    }

    /// Find the index of the next branch segment start in the given direction.
    /// A segment start is the first child of a branch point.
    ///
    /// "up" walks the visible parent chain; "down" walks visible children
    /// (always following the first child).
    fn find_branch_segment_start(&self, direction: Direction) -> isize {
        let Some(selected_id) = self.selected_entry_id() else {
            return self.selected_index;
        };

        let index_by_entry_id: HashMap<&str, usize> = self
            .filtered_nodes
            .iter()
            .enumerate()
            .map(|(index, position)| (self.flat_nodes[*position].node.entry.id(), index))
            .collect();
        let mut current_id = selected_id;
        if direction == Direction::Down {
            loop {
                let children = self
                    .visible_children_map
                    .get(&Some(current_id.clone()))
                    .cloned()
                    .unwrap_or_default();
                if children.is_empty() {
                    return index_by_entry_id[current_id.as_str()] as isize;
                }
                if children.len() > 1 {
                    return index_by_entry_id[children[0].as_str()] as isize;
                }
                current_id = children[0].clone();
            }
        }

        // direction === "up"
        loop {
            let Some(Some(parent_id)) = self.visible_parent_map.get(&current_id).cloned() else {
                return index_by_entry_id[current_id.as_str()] as isize;
            };
            let children = self
                .visible_children_map
                .get(&Some(parent_id.clone()))
                .cloned()
                .unwrap_or_default();
            if children.len() > 1 {
                let segment_start = index_by_entry_id[current_id.as_str()] as isize;
                if segment_start < self.selected_index {
                    return segment_start;
                }
            }
            current_id = parent_id;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Up,
    Down,
}

impl Component for TreeList {
    fn invalidate(&mut self) {}

    fn render(&mut self, width: usize) -> Vec<Line> {
        let theme_instance = theme();
        let mut lines: Vec<String> = Vec::new();

        if self.filtered_nodes.is_empty() {
            lines.push(truncate_to_width(
                &theme_instance.fg(ThemeColor::Muted, "  No entries found"),
                width,
            ));
            lines.push(truncate_to_width(
                &theme_instance.fg(
                    ThemeColor::Muted,
                    &format!("  (0/0){}", self.get_status_labels()),
                ),
                width,
            ));
            return shared_lines(lines);
        }

        let start_index = (self.selected_index - (self.max_visible_lines / 2) as isize)
            .min(self.filtered_nodes.len() as isize - self.max_visible_lines as isize)
            .max(0) as usize;
        let end_index = (start_index + self.max_visible_lines).min(self.filtered_nodes.len());

        let mut rendered_rows: Vec<HorizontalViewportRow> = Vec::new();
        for index in start_index..end_index {
            let flat_node = &self.flat_nodes[self.filtered_nodes[index]];
            let entry_id = flat_node.node.entry.id().to_string();
            let is_selected = index as isize == self.selected_index;

            // Build line: cursor + prefix + path marker + label + content
            let cursor = if is_selected {
                theme_instance.fg(ThemeColor::Accent, "› ")
            } else {
                "  ".to_string()
            };

            // If multiple roots, shift display (roots at 0, not 1)
            let display_indent = if self.multiple_roots {
                flat_node.indent.saturating_sub(1)
            } else {
                flat_node.indent
            };

            // Build prefix with gutters at their correct positions
            // Each gutter has a position (displayIndent where its connector was shown)
            let connector = if flat_node.show_connector && !flat_node.is_virtual_root_child {
                if flat_node.is_last {
                    "└─ "
                } else {
                    "├─ "
                }
            } else {
                ""
            };
            let connector_position = if connector.is_empty() {
                -1
            } else {
                display_indent as isize - 1
            };

            // Build prefix char by char, placing gutters and connector at their positions
            let total_chars = display_indent * 3;
            let mut prefix = String::new();
            let is_folded = self.folded_nodes.contains(&entry_id);
            for position in 0..total_chars {
                let level = position / 3;
                let position_in_level = position % 3;

                // Check if there's a gutter at this level
                let gutter = flat_node
                    .gutters
                    .iter()
                    .find(|gutter| gutter.position == level);
                if let Some(gutter) = gutter {
                    if position_in_level == 0 {
                        prefix.push(if gutter.show { '│' } else { ' ' });
                    } else {
                        prefix.push(' ');
                    }
                } else if !connector.is_empty() && level as isize == connector_position {
                    // Connector at this level, with fold indicator
                    if position_in_level == 0 {
                        prefix.push(if flat_node.is_last { '└' } else { '├' });
                    } else if position_in_level == 1 {
                        let foldable = self.is_foldable(&entry_id);
                        prefix.push(if is_folded {
                            '⊞'
                        } else if foldable {
                            '⊟'
                        } else {
                            '─'
                        });
                    } else {
                        prefix.push(' ');
                    }
                } else {
                    prefix.push(' ');
                }
            }

            // Fold marker for nodes without connectors (roots)
            let shows_fold_in_connector =
                flat_node.show_connector && !flat_node.is_virtual_root_child;
            let fold_marker = if is_folded && !shows_fold_in_connector {
                theme_instance.fg(ThemeColor::Accent, "⊞ ")
            } else {
                String::new()
            };

            // Active path marker - shown right before the entry text
            let is_on_active_path = self.active_path_ids.contains(&entry_id);
            let path_marker = if is_on_active_path {
                theme_instance.fg(ThemeColor::Accent, "• ")
            } else {
                String::new()
            };

            let label = match flat_node.node.label.as_deref() {
                Some(label) if !label.is_empty() => {
                    theme_instance.fg(ThemeColor::Warning, &format!("[{label}] "))
                }
                _ => String::new(),
            };
            let label_timestamp = match (
                self.show_label_timestamps,
                flat_node.node.label.as_deref(),
                flat_node.node.label_timestamp.as_deref(),
            ) {
                (true, Some(label), Some(timestamp))
                    if !label.is_empty() && !timestamp.is_empty() =>
                {
                    theme_instance.fg(
                        ThemeColor::Muted,
                        &format!("{} ", format_label_timestamp(timestamp)),
                    )
                }
                _ => String::new(),
            };
            let content = self.get_entry_display_text(&flat_node.node, is_selected);
            let prefix_part =
                theme_instance.fg(ThemeColor::Dim, &prefix) + &fold_marker + &path_marker;
            let anchor_col = visible_width(&prefix_part);
            let mut gutter = cursor;
            let mut body = prefix_part + &label + &label_timestamp + &content;
            if is_selected {
                gutter = theme_instance.bg(ThemeBg::SelectedBg, &gutter);
                body = theme_instance.bg(ThemeBg::SelectedBg, &body);
            }
            let body_width = visible_width(&body);
            rendered_rows.push(HorizontalViewportRow {
                gutter,
                body,
                anchor_col,
                body_width,
                is_selected,
            });
        }

        lines.extend(render_horizontal_viewport(&rendered_rows, width));
        lines.push(truncate_to_width(
            &theme_instance.fg(
                ThemeColor::Muted,
                &format!(
                    "  ({}/{}){}",
                    self.selected_index + 1,
                    self.filtered_nodes.len(),
                    self.get_status_labels()
                ),
            ),
            width,
        ));

        shared_lines(lines)
    }

    fn handle_input(&mut self, key_data: &str) {
        let length = self.filtered_nodes.len() as isize;
        if keybindings_match(key_data, "tui.select.up") {
            self.selected_index = if self.selected_index == 0 {
                length - 1
            } else {
                self.selected_index - 1
            };
        } else if keybindings_match(key_data, "tui.select.down") {
            self.selected_index = if self.selected_index == length - 1 {
                0
            } else {
                self.selected_index + 1
            };
        } else if keybindings_match(key_data, "app.tree.foldOrUp") {
            let current_id = self.selected_entry_id();
            match current_id {
                Some(id) if self.is_foldable(&id) && !self.folded_nodes.contains(&id) => {
                    self.folded_nodes.insert(id);
                    self.apply_filter();
                }
                _ => self.selected_index = self.find_branch_segment_start(Direction::Up),
            }
        } else if keybindings_match(key_data, "app.tree.unfoldOrDown") {
            let current_id = self.selected_entry_id();
            match current_id {
                Some(id) if self.folded_nodes.contains(&id) => {
                    self.folded_nodes.remove(&id);
                    self.apply_filter();
                }
                _ => self.selected_index = self.find_branch_segment_start(Direction::Down),
            }
        } else if keybindings_match(key_data, "tui.editor.cursorLeft")
            || keybindings_match(key_data, "tui.select.pageUp")
        {
            // Page up
            self.selected_index = (self.selected_index - self.max_visible_lines as isize).max(0);
        } else if keybindings_match(key_data, "tui.editor.cursorRight")
            || keybindings_match(key_data, "tui.select.pageDown")
        {
            // Page down
            self.selected_index =
                (self.selected_index + self.max_visible_lines as isize).min(length - 1);
        } else if keybindings_match(key_data, "tui.select.confirm") {
            if let Some(id) = self.selected_entry_id()
                && let Some(on_select) = self.on_select.as_mut()
            {
                on_select(&id);
            }
        } else if keybindings_match(key_data, "app.message.copy") {
            self.copy_selected();
        } else if keybindings_match(key_data, "tui.select.cancel") {
            if !self.search_query.is_empty() {
                self.search_query.clear();
                self.folded_nodes.clear();
                self.apply_filter();
            } else if let Some(on_cancel) = self.on_cancel.as_mut() {
                on_cancel();
            }
        } else if keybindings_match(key_data, "app.tree.filter.default") {
            // Direct filter: default
            self.set_filter_mode(FilterMode::Default);
        } else if keybindings_match(key_data, "app.tree.filter.noTools") {
            // Toggle filter: no-tools ↔ default
            self.toggle_filter_mode(FilterMode::NoTools);
        } else if keybindings_match(key_data, "app.tree.filter.userOnly") {
            // Toggle filter: user-only ↔ default
            self.toggle_filter_mode(FilterMode::UserOnly);
        } else if keybindings_match(key_data, "app.tree.filter.labeledOnly") {
            // Toggle filter: labeled-only ↔ default
            self.toggle_filter_mode(FilterMode::LabeledOnly);
        } else if keybindings_match(key_data, "app.tree.filter.all") {
            // Toggle filter: all ↔ default
            self.toggle_filter_mode(FilterMode::All);
        } else if keybindings_match(key_data, "app.tree.filter.cycleBackward") {
            // Cycle filter backwards
            let current_index = FILTER_MODES
                .iter()
                .position(|mode| *mode == self.filter_mode)
                .unwrap_or(0);
            self.set_filter_mode(
                FILTER_MODES[(current_index + FILTER_MODES.len() - 1) % FILTER_MODES.len()],
            );
        } else if keybindings_match(key_data, "app.tree.filter.cycleForward") {
            // Cycle filter forwards: default → no-tools → user-only → labeled-only → all → default
            let current_index = FILTER_MODES
                .iter()
                .position(|mode| *mode == self.filter_mode)
                .unwrap_or(0);
            self.set_filter_mode(FILTER_MODES[(current_index + 1) % FILTER_MODES.len()]);
        } else if keybindings_match(key_data, "tui.editor.deleteCharBackward") {
            if !self.search_query.is_empty() {
                // `slice(0, -1)` drops one UTF-16 code unit.
                let query = js_slice(&self.search_query, js_length(&self.search_query) - 1);
                self.search_query = query.to_string();
                self.folded_nodes.clear();
                self.apply_filter();
            }
        } else if keybindings_match(key_data, "app.tree.editLabel") {
            if let Some(node) = self.get_selected_node() {
                self.pending_label_edit = Some((node.entry.id().to_string(), node.label.clone()));
            }
        } else if keybindings_match(key_data, "app.tree.toggleLabelTimestamp") {
            self.show_label_timestamps = !self.show_label_timestamps;
        } else {
            let has_control_chars = key_data.chars().any(|character| {
                let code = character as u32;
                code < 32 || code == 0x7f || (0x80..=0x9f).contains(&code)
            });
            if !has_control_chars && !key_data.is_empty() {
                self.search_query.push_str(key_data);
                self.folded_nodes.clear();
                self.apply_filter();
            }
        }
    }
}

impl TreeList {
    fn set_filter_mode(&mut self, mode: FilterMode) {
        self.filter_mode = mode;
        self.folded_nodes.clear();
        self.apply_filter();
    }

    fn toggle_filter_mode(&mut self, mode: FilterMode) {
        let next = if self.filter_mode == mode {
            FilterMode::Default
        } else {
            mode
        };
        self.set_filter_mode(next);
    }
}

// --- entry text helpers -------------------------------------------------------

fn extract_content(content: Option<&Value>) -> String {
    js_slice(&extract_full_content(content), 200).to_string()
}

fn extract_full_content(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => {
            let mut result = String::new();
            for block in blocks {
                if block.get("type").and_then(Value::as_str) == Some("text") {
                    result.push_str(
                        block
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    );
                }
            }
            result
        }
        _ => String::new(),
    }
}

fn get_entry_copy_text(node: &TreeNodeData) -> Option<String> {
    let text: Option<String> = match &node.entry {
        SessionEntry::Message(message_entry) => {
            let message = &message_entry.message;
            if message_role(message) == "bashExecution" {
                message
                    .get("command")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            } else if message.get("content").is_some() {
                let text = extract_full_content(message_field(message, "content"));
                if text.is_empty() && message_role(message) == "assistant" {
                    message
                        .get("errorMessage")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                } else {
                    Some(text)
                }
            } else {
                None
            }
        }
        SessionEntry::CustomMessage(custom) => Some(extract_full_content(Some(&custom.content))),
        SessionEntry::Compaction(compaction) => Some(compaction.summary.clone()),
        SessionEntry::BranchSummary(summary) => Some(summary.summary.clone()),
        _ => None,
    };

    // `text?.trim() ? text : undefined`
    text.filter(|value| !value.trim().is_empty())
}

fn has_text_content(content: Option<&Value>) -> bool {
    match content {
        Some(Value::String(text)) => !text.trim().is_empty(),
        Some(Value::Array(blocks)) => blocks.iter().any(|block| {
            block.get("type").and_then(Value::as_str) == Some("text")
                && block
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.trim().is_empty())
        }),
        _ => false,
    }
}

fn shorten_path(path: &str) -> String {
    let home = std::env::var("HOME")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::env::var("USERPROFILE")
                .ok()
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_default();
    if !home.is_empty()
        && let Some(rest) = path.strip_prefix(&home)
    {
        return format!("~{rest}");
    }
    path.to_string()
}

fn argument_path(arguments: &Map<String, Value>) -> String {
    // `String(args.path || args.file_path || "")`
    let path = js_string(arguments.get("path"));
    if !path.is_empty() {
        return shorten_path(&path);
    }
    let file_path = js_string(arguments.get("file_path"));
    shorten_path(&file_path)
}

fn format_tool_call(name: &str, arguments: &Map<String, Value>) -> String {
    match name {
        "read" | "read_minified" => {
            let path = argument_path(arguments);
            let offset = arguments.get("offset").and_then(Value::as_f64);
            let limit = arguments.get("limit").and_then(Value::as_f64);
            let mut display = path;
            if offset.is_some() || limit.is_some() {
                let start = offset.unwrap_or(1.0);
                let end = limit.map(|limit| start + limit - 1.0);
                display.push_str(&format!(
                    ":{}{}",
                    to_js_string(start),
                    // `end ? \`-${end}\` : ""` — a computed 0 is falsy in JavaScript.
                    match end {
                        Some(end) if end != 0.0 => format!("-{}", to_js_string(end)),
                        _ => String::new(),
                    }
                ));
            }
            format!(
                "[{}: {display}]",
                if name == "read" {
                    "read"
                } else {
                    "read minified"
                }
            )
        }
        "write" => format!("[write: {}]", argument_path(arguments)),
        "patch" => format!("[patch: {}]", argument_path(arguments)),
        "patch_minified" => format!("[patch minified: {}]", argument_path(arguments)),
        "multi_patch_minified" => {
            let path = argument_path(arguments);
            let count = match arguments.get("edits") {
                Some(Value::Array(edits)) => edits.len(),
                _ => 0,
            };
            let suffix = if count > 0 {
                format!(" ({count})")
            } else {
                String::new()
            };
            format!("[multi patch minified: {path}{suffix}]")
        }
        "bash" => {
            let raw_command = js_string(arguments.get("command"));
            let command = js_slice(normalize(&raw_command).as_str(), 50).to_string();
            format!(
                "[bash: {command}{}]",
                if js_length(&raw_command) > 50 {
                    "..."
                } else {
                    ""
                }
            )
        }
        "grep" => {
            let pattern = js_string(arguments.get("pattern"));
            let path = argument_or_dot(arguments);
            format!("[grep: /{pattern}/ in {path}]")
        }
        "find_filesystem" => {
            let pattern = js_string(arguments.get("pattern"));
            let path = argument_or_dot(arguments);
            format!("[find_filesystem: {pattern} in {path}]")
        }
        "find_codebase" => {
            let query = js_string(arguments.get("query"));
            let path = argument_or_dot(arguments);
            format!("[find_codebase: /{query}/ in {path}]")
        }
        "ls" => format!("[ls: {}]", argument_or_dot(arguments)),
        _ => {
            // Custom tool - show name and truncated JSON args
            let serialized = js_stringify(arguments);
            let arguments_text = js_slice(&serialized, 40);
            format!(
                "[{name}: {arguments_text}{}]",
                if js_length(&serialized) > 40 {
                    "..."
                } else {
                    ""
                }
            )
        }
    }
}

/// `shortenPath(String(args.path || "."))`
fn argument_or_dot(arguments: &Map<String, Value>) -> String {
    let path = js_string(arguments.get("path"));
    shorten_path(if path.is_empty() { "." } else { &path })
}

fn format_label_timestamp(timestamp: &str) -> String {
    use chrono::{Datelike, Local, Timelike};

    let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(timestamp) else {
        // `new Date("nonsense")` is an Invalid Date, whose getters all return NaN.
        return "aN/NaN/NaN NaN:NaN".to_string();
    };
    let date = parsed.with_timezone(&Local);
    let now = Local::now();
    let time = format!("{:02}:{:02}", date.hour(), date.minute());

    if date.year() == now.year() && date.month() == now.month() && date.day() == now.day() {
        return time;
    }

    let month = date.month();
    let day = date.day();
    if date.year() == now.year() {
        return format!("{month}/{day} {time}");
    }

    // `date.getFullYear().toString().slice(-2)`
    let year_text = date.year().to_string();
    let year = js_slice_end(&year_text, 2);
    format!("{year}/{month}/{day} {time}")
}

/// `String.prototype.slice(-n)` — the last `n` UTF-16 code units.
fn js_slice_end(text: &str, count: usize) -> &str {
    let length = js_length(text);
    if length <= count {
        return text;
    }
    let mut units = 0usize;
    for (offset, character) in text.char_indices() {
        if units >= length - count {
            return &text[offset..];
        }
        units += character.len_utf16();
    }
    ""
}

fn now_iso_string() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// --- surrounding components ---------------------------------------------------

/// Component that displays the current search query
struct SearchLine {
    tree_list: Rc<RefCell<TreeList>>,
}

impl Component for SearchLine {
    fn invalidate(&mut self) {}

    fn render(&mut self, width: usize) -> Vec<Line> {
        let theme_instance = theme();
        let tree_list = self.tree_list.borrow();
        let query = tree_list.get_search_query();
        if !query.is_empty() {
            return vec![Line::from(truncate_to_width(
                &format!(
                    "  {} {}",
                    theme_instance.fg(ThemeColor::Muted, "Type to search:"),
                    theme_instance.fg(ThemeColor::Accent, query)
                ),
                width,
            ))];
        }
        vec![Line::from(truncate_to_width(
            &format!(
                "  {}",
                theme_instance.fg(ThemeColor::Muted, "Type to search:")
            ),
            width,
        ))]
    }

    fn handle_input(&mut self, _key_data: &str) {}
}

/// Component that renders tree help as semantic rows with chunk-aware wrapping
struct TreeHelp;

impl Component for TreeHelp {
    fn invalidate(&mut self) {}

    fn render(&mut self, width: usize) -> Vec<Line> {
        let theme_instance = theme();
        let items: Vec<String> = TREE_HELP_ITEMS
            .iter()
            .map(|help| {
                let text = format_help_keys(help.keys);
                if text.is_empty() {
                    return help.label.to_string();
                }
                if help.label_first {
                    format!("{} {text}", help.label)
                } else {
                    format!("{text} {}", help.label)
                }
            })
            .collect();

        let available_width = width.max(1);
        let indent = "  ";
        let separator = " · ";
        let mut lines: Vec<String> = Vec::new();
        let mut current_line = String::new();

        for item in items {
            let indented = format!("{indent}{item}");
            let candidate = if !current_line.is_empty() {
                format!("{current_line}{separator}{item}")
            } else if visible_width(&indented) <= available_width {
                indented.clone()
            } else {
                item.clone()
            };
            if current_line.is_empty() || visible_width(&candidate) <= available_width {
                current_line = candidate;
                continue;
            }

            lines.extend(wrap_text_with_ansi(
                current_line.trim_end(),
                available_width,
            ));
            current_line = if visible_width(&indented) <= available_width {
                indented
            } else {
                item
            };
        }

        if !current_line.is_empty() {
            lines.extend(wrap_text_with_ansi(
                current_line.trim_end(),
                available_width,
            ));
        }

        lines
            .into_iter()
            .map(|line| Line::from(theme_instance.fg(ThemeColor::Muted, &line)))
            .collect()
    }
}

struct TreeHelpItem {
    keys: &'static [&'static str],
    label: &'static str,
    label_first: bool,
}

const TREE_HELP_ITEMS: [TreeHelpItem; 8] = [
    TreeHelpItem {
        keys: &["tui.select.up", "tui.select.down"],
        label: "move",
        label_first: false,
    },
    TreeHelpItem {
        keys: &["tui.editor.cursorLeft", "tui.editor.cursorRight"],
        label: "page",
        label_first: false,
    },
    TreeHelpItem {
        keys: &["app.tree.foldOrUp", "app.tree.unfoldOrDown"],
        label: "branch",
        label_first: false,
    },
    TreeHelpItem {
        keys: &["app.message.copy"],
        label: "copy",
        label_first: false,
    },
    TreeHelpItem {
        keys: &["app.tree.editLabel"],
        label: "label",
        label_first: false,
    },
    TreeHelpItem {
        keys: &["app.tree.toggleLabelTimestamp"],
        label: "label time",
        label_first: false,
    },
    TreeHelpItem {
        keys: &[
            "app.tree.filter.default",
            "app.tree.filter.noTools",
            "app.tree.filter.userOnly",
            "app.tree.filter.labeledOnly",
            "app.tree.filter.all",
        ],
        label: "filters",
        label_first: true,
    },
    TreeHelpItem {
        keys: &[
            "app.tree.filter.cycleForward",
            "app.tree.filter.cycleBackward",
        ],
        label: "cycle",
        label_first: true,
    },
];

fn format_help_keys(keybindings: &[&str]) -> String {
    let mut keys: Vec<String> = Vec::new();
    for keybinding in keybindings {
        if let Some(key) = keybinding_keys(keybinding).into_iter().next() {
            keys.push(key);
        }
    }
    if keys.is_empty() {
        return String::new();
    }

    let mut text = format_key_text(&compact_raw_keys(&keys), KeyTextFormatOptions::default());
    for (pattern, replacement) in help_key_replacements() {
        text = pattern.replace_all(&text, *replacement).into_owned();
    }
    text
}

/// The word-boundary replacements of `formatHelpKeys`, in source order.
fn help_key_replacements() -> &'static [(Regex, &'static str)] {
    static REPLACEMENTS: std::sync::OnceLock<Vec<(Regex, &'static str)>> =
        std::sync::OnceLock::new();
    REPLACEMENTS.get_or_init(|| {
        vec![
            (Regex::new(r"\bpageUp\b").unwrap(), "pgup"),
            (Regex::new(r"\bpageDown\b").unwrap(), "pgdn"),
            (Regex::new(r"\bup\b").unwrap(), "↑"),
            (Regex::new(r"\bdown\b").unwrap(), "↓"),
            (Regex::new(r"\bleft\b").unwrap(), "←"),
            (Regex::new(r"\bright\b").unwrap(), "→"),
        ]
    })
}

fn compact_raw_keys(keys: &[String]) -> String {
    if keys.len() == 1 {
        return keys[0].clone();
    }

    let parts: Vec<(String, String)> = keys
        .iter()
        .map(|key| match key.rfind('+') {
            None => (String::new(), key.clone()),
            Some(index) => (key[..index + 1].to_string(), key[index + 1..].to_string()),
        })
        .collect();
    let prefix = parts[0].0.clone();
    if !prefix.is_empty() && parts.iter().all(|part| part.0 == prefix) {
        format!(
            "{prefix}{}",
            parts
                .iter()
                .map(|part| part.1.clone())
                .collect::<Vec<_>>()
                .join("/")
        )
    } else {
        keys.join("/")
    }
}

/// Label input component shown when editing a label
struct LabelInput {
    input: Input,
    entry_id: String,
    /// `onSubmit`/`onCancel` call back into the selector; the port records the
    /// request and the selector drains it after dispatch.
    pending_submit: Option<(String, Option<String>)>,
    pending_cancel: bool,

    // Focusable implementation - propagate to input for IME cursor positioning
    focused: bool,
}

impl LabelInput {
    fn new(entry_id: &str, current_label: Option<&str>) -> Self {
        let mut input = Input::new();
        if let Some(label) = current_label
            && !label.is_empty()
        {
            input.set_value(label);
        }
        Self {
            input,
            entry_id: entry_id.to_string(),
            pending_submit: None,
            pending_cancel: false,
            focused: false,
        }
    }
}

impl Component for LabelInput {
    fn invalidate(&mut self) {}

    fn render(&mut self, width: usize) -> Vec<Line> {
        let theme_instance = theme();
        let mut lines: Vec<String> = Vec::new();
        let indent = "  ";
        let available_width = width.saturating_sub(indent.len());
        lines.push(truncate_to_width(
            &format!(
                "{indent}{}",
                theme_instance.fg(ThemeColor::Muted, "Label (empty to remove):")
            ),
            width,
        ));
        lines.extend(
            self.input
                .render(available_width)
                .into_iter()
                .map(|line| truncate_to_width(&format!("{indent}{line}"), width)),
        );
        lines.push(truncate_to_width(
            &format!(
                "{indent}{}  {}",
                key_hint("tui.select.confirm", "save"),
                key_hint("tui.select.cancel", "cancel")
            ),
            width,
        ));
        shared_lines(lines)
    }

    fn handle_input(&mut self, key_data: &str) {
        if keybindings_match(key_data, "tui.select.confirm") {
            let value = self.input.get_value().trim().to_string();
            self.pending_submit = Some((
                self.entry_id.clone(),
                if value.is_empty() { None } else { Some(value) },
            ));
        } else if keybindings_match(key_data, "tui.select.cancel") {
            self.pending_cancel = true;
        } else {
            self.input.handle_input(key_data);
        }
    }

    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        Some(self)
    }
}

impl Focusable for LabelInput {
    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.input.set_focused(focused);
    }
}

/// The optional trailing parameters of the TypeScript constructor.
#[derive(Default)]
pub struct TreeSelectorOptions {
    /// Invoked when a label was set or cleared.
    pub on_label_change: Option<LabelChangeCallback>,
    /// Entry the cursor starts on; defaults to the current leaf.
    pub initial_selected_id: Option<String>,
    /// Filter mode the selector opens with.
    pub initial_filter_mode: Option<FilterMode>,
}

/// Component that renders a session tree selector for navigation
pub struct TreeSelectorComponent {
    container: Container,
    tree_list: Rc<RefCell<TreeList>>,
    label_input: Option<Rc<RefCell<LabelInput>>>,
    label_input_container: Rc<RefCell<Container>>,
    tree_container: Rc<RefCell<Container>>,
    on_label_change_callback: Option<LabelChangeCallback>,
    /// Invoked with the text of the copied entry.
    pub on_copy: Option<CopyCallback>,
    /// `tree.length === 0` schedules `onCancel` in TypeScript; timers never call
    /// back in this port, so the caller polls this flag instead.
    empty: bool,

    // Focusable implementation - propagate to labelInput when active for IME cursor positioning
    focused: bool,
}

impl TreeSelectorComponent {
    /// New selector over `tree`.
    pub fn new(
        tree: &[SessionTreeNode],
        current_leaf_id: Option<&str>,
        terminal_height: usize,
        on_select: TreeSelectCallback,
        on_cancel: Box<dyn FnMut()>,
        options: TreeSelectorOptions,
    ) -> Self {
        let theme_instance = theme();
        let max_visible_lines = (terminal_height / 2).max(5);

        let mut tree_list = TreeList::new(
            tree,
            current_leaf_id,
            max_visible_lines,
            options.initial_selected_id.as_deref(),
            options.initial_filter_mode,
        );
        tree_list.on_select = Some(on_select);
        tree_list.on_cancel = Some(on_cancel);
        let tree_list = Rc::new(RefCell::new(tree_list));

        let tree_container = Rc::new(RefCell::new(Container::new()));
        tree_container
            .borrow_mut()
            .add_child(Rc::clone(&tree_list) as ComponentRef);

        let label_input_container = Rc::new(RefCell::new(Container::new()));

        let mut container = Container::new();
        container.add_child(component_ref(Spacer::new(1)));
        container.add_child(component_ref(DynamicBorder::new(None)));
        container.add_child(component_ref(Text::new(
            theme_instance.bold("  Session Tree"),
            1,
            0,
        )));
        container.add_child(component_ref(TreeHelp));
        container.add_child(component_ref(SearchLine {
            tree_list: Rc::clone(&tree_list),
        }));
        container.add_child(component_ref(DynamicBorder::new(None)));
        container.add_child(component_ref(Spacer::new(1)));
        container.add_child(Rc::clone(&tree_container) as ComponentRef);
        container.add_child(Rc::clone(&label_input_container) as ComponentRef);
        container.add_child(component_ref(Spacer::new(1)));
        container.add_child(component_ref(DynamicBorder::new(None)));

        Self {
            container,
            tree_list,
            label_input: None,
            label_input_container,
            tree_container,
            on_label_change_callback: options.on_label_change,
            on_copy: None,
            empty: tree.is_empty(),
            focused: false,
        }
    }

    /// The tree list; `tree.length === 0` means the caller must cancel.
    pub fn get_tree_list(&self) -> &Rc<RefCell<TreeList>> {
        &self.tree_list
    }

    /// Whether the selector was opened on an empty tree.
    pub fn is_empty(&self) -> bool {
        self.empty
    }

    fn show_label_input(&mut self, entry_id: &str, current_label: Option<&str>) {
        let label_input = Rc::new(RefCell::new(LabelInput::new(entry_id, current_label)));

        // Propagate current focused state to the new labelInput
        label_input.borrow_mut().set_focused(self.focused);

        self.tree_container.borrow_mut().clear();
        self.label_input_container.borrow_mut().clear();
        self.label_input_container
            .borrow_mut()
            .add_child(Rc::clone(&label_input) as ComponentRef);
        self.label_input = Some(label_input);
    }

    fn hide_label_input(&mut self) {
        self.label_input = None;
        self.label_input_container.borrow_mut().clear();
        self.tree_container.borrow_mut().clear();
        self.tree_container
            .borrow_mut()
            .add_child(Rc::clone(&self.tree_list) as ComponentRef);
    }
}

impl Component for TreeSelectorComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.container.render(width)
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }

    fn handle_input(&mut self, key_data: &str) {
        if let Some(label_input) = self.label_input.clone() {
            label_input.borrow_mut().handle_input(key_data);
            let submitted = label_input.borrow_mut().pending_submit.take();
            let cancelled = std::mem::take(&mut label_input.borrow_mut().pending_cancel);
            if let Some((entry_id, label)) = submitted {
                self.tree_list
                    .borrow_mut()
                    .update_node_label(&entry_id, label.as_deref(), None);
                if let Some(on_label_change) = self.on_label_change_callback.as_mut() {
                    on_label_change(&entry_id, label.as_deref());
                }
                self.hide_label_input();
            } else if cancelled {
                self.hide_label_input();
            }
        } else {
            self.tree_list.borrow_mut().handle_input(key_data);
            let copied = self.tree_list.borrow_mut().pending_copy.take();
            let label_edit = self.tree_list.borrow_mut().pending_label_edit.take();
            if let Some(text) = copied
                && let Some(on_copy) = self.on_copy.as_mut()
            {
                on_copy(text.as_deref());
            }
            if let Some((entry_id, current_label)) = label_edit {
                self.show_label_input(&entry_id, current_label.as_deref());
            }
        }
    }

    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        Some(self)
    }
}

impl Focusable for TreeSelectorComponent {
    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        // Propagate to labelInput when it's active
        if let Some(label_input) = self.label_input.as_ref() {
            label_input.borrow_mut().set_focused(focused);
        }
    }
}
