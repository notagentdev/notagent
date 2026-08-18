//! An MCP tool as this port's registry holds one (v0.1.22).
//!
//! `ToolName` here is a closed enum of the built-ins, inherited from the
//! TypeScript original's closed union, and modes are typed against it. MCP tool
//! names are not known until a server has been asked. Widening the enum would
//! push knowledge of servers that may not exist into every mode file, so MCP
//! tools live beside the enum-named built-ins as ordinary name-keyed registry
//! entries instead: outside the mode allowlist, gated by the permission chain,
//! which matches on the tool name as text.
//!
//! A built-in always wins a name collision. A server that offered a tool
//! qualifying to `read` or `bash` would otherwise take over a tool the model
//! trusts, which is a hijack rather than a configuration choice.

use std::sync::Arc;

use notagent_agent::types::{
    AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::types::{ConstrainedSampling, TextContent, TextOrImageContent};
use notagent_tui::tui::ComponentRef;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::core::mcp::call::{McpRecovery, call_tool};
use crate::core::mcp::manager::{McpManager, McpToolInfo};
use crate::core::mcp::output::convert_result;
use crate::core::tools::mcp_render::{format_mcp_call, format_mcp_result};
use crate::core::tools::render_utils::call_title;
use crate::core::tools::tool_definition::{
    ToolContext, ToolDefinition, ToolRenderContext, ToolRenderResult, ToolRenderResultOptions,
    render_text_call, render_text_result,
};
use crate::modes::interactive::theme::theme::Theme;

/// One MCP tool, bound to the manager that owns its server.
pub struct McpToolDefinition {
    info: McpToolInfo,
    manager: Arc<McpManager>,
    accepts_images: bool,
    parameters: Value,
}

pub fn create_mcp_tool_definition(
    info: McpToolInfo,
    manager: Arc<McpManager>,
    accepts_images: bool,
) -> McpToolDefinition {
    // A server may describe its input as anything; the registry needs an object
    // schema, and an empty one accepts whatever the model sends rather than
    // refusing every call.
    let parameters = match &info.input_schema {
        Value::Object(_) => info.input_schema.clone(),
        _ => serde_json::json!({ "type": "object", "properties": {} }),
    };
    McpToolDefinition {
        info,
        manager,
        accepts_images,
        parameters,
    }
}

impl ToolDefinition for McpToolDefinition {
    fn name(&self) -> &str {
        &self.info.qualified_name
    }

    fn label(&self) -> &str {
        &self.info.tool_name
    }

    fn description(&self) -> &str {
        &self.info.description
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn constrained_sampling(&self) -> Option<&ConstrainedSampling> {
        None
    }

    fn render_call(
        &self,
        args: &Value,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        // The server is the block title and the tool its subject, so a row
        // reads `GITHUB create_issue repo: "acme/app"` — the same shape the
        // built-ins have, rather than a qualified name and a JSON blob.
        let title = call_title(theme, self.info.server.as_str());
        let call = format_mcp_call(&self.info.tool_name, args, context.args_complete, theme);
        Some(render_text_call(context, &format!("{title}{call}")))
    }

    fn render_result(
        &self,
        result: ToolRenderResult<'_>,
        options: ToolRenderResultOptions,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        Some(render_text_result(
            context,
            &format_mcp_result(result, options.expanded, context.show_images, theme),
        ))
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        params: Value,
        signal: Option<CancellationToken>,
        _on_update: Option<AgentToolUpdateCallback>,
        _context: Option<ToolContext>,
    ) -> BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>> {
        Box::pin(async move {
            if matches!(&signal, Some(signal) if signal.is_cancelled()) {
                return Err(ToolExecutionError::new("Operation aborted"));
            }
            let arguments = match params {
                Value::Object(map) => map,
                _ => serde_json::Map::new(),
            };

            // The signal travels into the call rather than being checked only
            // here: a server that takes a minute would otherwise keep the turn
            // waiting long after the user ended it, and would finish work for a
            // client that is no longer listening.
            let outcome = call_tool(&self.manager, &self.info, arguments, signal.as_ref())
                .await
                .map_err(|error| ToolExecutionError::new(error.to_string()))?;
            let output = convert_result(&outcome.result, self.accepts_images);

            let mut content = output.content;
            if output.truncated || outcome.recovery != McpRecovery::None {
                let mut notes = Vec::new();
                if output.truncated {
                    notes.push("the server's output was truncated".to_owned());
                }
                match outcome.recovery {
                    McpRecovery::RetriedInPlace => {
                        notes.push("the call was retried after a connection hiccup".to_owned());
                    }
                    McpRecovery::Reconnected => {
                        notes.push(format!(
                            "the connection to `{}` was re-established and the call repeated, \
                             which may have run it twice",
                            self.info.server
                        ));
                    }
                    McpRecovery::None => {}
                }
                content.push(TextOrImageContent::Text(TextContent::new(format!(
                    "\n[{}]",
                    notes.join("; ")
                ))));
            }

            if output.is_error {
                return Err(ToolExecutionError::new(text_of(&content)));
            }
            Ok(AgentToolResult {
                content,
                details: None,
                usage: None,
                added_tool_names: None,
                terminate: None,
            })
        })
    }
}

fn text_of(content: &[TextOrImageContent]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            TextOrImageContent::Text(text) => Some(text.text.clone()),
            TextOrImageContent::Image(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Builds the registry entries for a set of discovered tools.
///
/// `taken` is every name already spoken for — the built-ins first, then each
/// tool accepted here — so a collision drops the newcomer rather than the
/// incumbent.
pub fn mcp_tool_definitions(
    tools: Vec<McpToolInfo>,
    manager: &Arc<McpManager>,
    accepts_images: bool,
    taken: &mut std::collections::BTreeSet<String>,
) -> Vec<(String, Arc<dyn ToolDefinition>)> {
    let mut definitions: Vec<(String, Arc<dyn ToolDefinition>)> = Vec::new();
    for info in tools {
        if !taken.insert(info.qualified_name.clone()) {
            continue;
        }
        let name = info.qualified_name.clone();
        definitions.push((
            name,
            Arc::new(create_mcp_tool_definition(
                info,
                Arc::clone(manager),
                accepts_images,
            )) as Arc<dyn ToolDefinition>,
        ));
    }
    definitions
}

/// An MCP tool is never offered to a subagent unless its parent had it, and the
/// delegation path already withholds everything a child may not start on its
/// own. This is the name predicate the registry uses to recognise one.
pub fn is_mcp_tool_name(name: &str) -> bool {
    name.starts_with("mcp__")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mcp::ServerName;
    use std::collections::BTreeSet;

    fn info(server: &str, tool: &str) -> McpToolInfo {
        McpToolInfo {
            qualified_name: format!("mcp__{server}__{tool}"),
            tool_name: tool.to_owned(),
            server: ServerName::from(server),
            description: String::new(),
            input_schema: serde_json::json!({ "type": "object" }),
        }
    }

    #[test]
    fn a_built_in_name_is_not_displaced() {
        let manager = Arc::new(McpManager::empty());
        // The built-ins claim their names first.
        let mut taken: BTreeSet<String> = crate::core::tools::ALL_TOOL_NAMES
            .iter()
            .map(|name| name.as_str().to_owned())
            .collect();
        let mut hijack = info("evil", "read");
        hijack.qualified_name = "read".to_owned();

        let definitions = mcp_tool_definitions(vec![hijack], &manager, true, &mut taken);

        assert!(definitions.is_empty());
    }

    #[test]
    fn two_servers_offering_the_same_tool_keep_separate_names() {
        let manager = Arc::new(McpManager::empty());
        let mut taken = BTreeSet::new();

        let definitions = mcp_tool_definitions(
            vec![info("a", "read"), info("b", "read")],
            &manager,
            true,
            &mut taken,
        );

        let names: Vec<&str> = definitions.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, vec!["mcp__a__read", "mcp__b__read"]);
    }

    #[test]
    fn the_same_tool_twice_is_registered_once() {
        let manager = Arc::new(McpManager::empty());
        let mut taken = BTreeSet::new();

        let definitions = mcp_tool_definitions(
            vec![info("a", "read"), info("a", "read")],
            &manager,
            true,
            &mut taken,
        );

        assert_eq!(definitions.len(), 1);
    }

    #[test]
    fn a_schema_that_is_not_an_object_becomes_one() {
        let manager = Arc::new(McpManager::empty());
        let mut broken = info("a", "read");
        broken.input_schema = serde_json::json!("not a schema");

        let definition = create_mcp_tool_definition(broken, manager, true);

        assert_eq!(definition.parameters()["type"], "object");
    }

    #[test]
    fn no_mcp_tool_can_reach_a_subagent() {
        // A child's roster is `child_tool_names(&mode) -> Vec<ToolName>`, drawn
        // from the mode's allowlist, and `ToolName` is the closed enum of the
        // built-ins. So this holds by construction rather than by a filter that
        // could be forgotten — and it is worth pinning, because an MCP tool a
        // child could start would be a server the user trusted for this session
        // being driven by an agent they never saw. If `ToolName` ever gains a
        // dynamic variant, this is what notices.
        for name in crate::core::tools::ALL_TOOL_NAMES {
            assert!(
                !is_mcp_tool_name(name.as_str()),
                "`{}` is reachable from a subagent's roster",
                name.as_str()
            );
        }
    }

    #[test]
    fn an_mcp_name_is_recognisable() {
        assert!(is_mcp_tool_name("mcp__files__read"));
        assert!(!is_mcp_tool_name("read"));
        assert!(!is_mcp_tool_name("bash"));
    }
}
