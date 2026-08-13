//! Splitting tools into immediate and transcript-loaded definitions.
//!
//! 1:1 port of `packages/ai/src/utils/deferred-tools.ts` (39 LOC).

use std::collections::{BTreeMap, BTreeSet};

use crate::types::{AssistantContent, Context, Message, Tool};

/// Result of [`split_deferred_tools`].
pub struct SplitTools {
    pub immediate: Vec<Tool>,
    pub deferred: BTreeMap<String, Tool>,
}

/// `splitDeferredTools(context, enabled, normalizeName?)`
///
/// The unique-tool map keeps insertion order in TS; the Rust port preserves it by
/// building a `Vec` of `(name, tool)` pairs and de-duplicating by normalized name.
pub fn split_deferred_tools(
    context: &Context,
    enabled: bool,
    normalize_name: impl Fn(&str) -> String,
) -> SplitTools {
    let mut unique_tools: Vec<(String, Tool)> = Vec::new();
    for tool in context.tools.iter().flatten() {
        let name = normalize_name(&tool.name);
        match unique_tools
            .iter_mut()
            .find(|(existing, _)| *existing == name)
        {
            Some((_, existing)) => *existing = tool.clone(),
            None => unique_tools.push((name, tool.clone())),
        }
    }

    if !enabled {
        return SplitTools {
            immediate: unique_tools.into_iter().map(|(_, tool)| tool).collect(),
            deferred: BTreeMap::new(),
        };
    }

    let mut deferred_names: BTreeSet<String> = BTreeSet::new();
    let mut used_names: BTreeSet<String> = BTreeSet::new();
    for message in &context.messages {
        match message {
            Message::Assistant(message) => {
                for block in &message.content {
                    if let AssistantContent::ToolCall(tool_call) = block {
                        used_names.insert(normalize_name(&tool_call.name));
                    }
                }
            }
            Message::ToolResult(message) => {
                for name in message.added_tool_names.iter().flatten() {
                    let normalized_name = normalize_name(name);
                    if !used_names.contains(&normalized_name) {
                        deferred_names.insert(normalized_name);
                    }
                }
            }
            Message::User(_) => {}
        }
    }

    let mut immediate = Vec::new();
    let mut deferred = BTreeMap::new();
    for (name, tool) in unique_tools {
        if deferred_names.contains(&name) {
            deferred.insert(name, tool);
        } else {
            immediate.push(tool);
        }
    }
    SplitTools {
        immediate,
        deferred,
    }
}

/// The identity normalizer used when the caller passes none.
pub fn identity_tool_name(name: &str) -> String {
    name.to_string()
}
