//! The synthetic `authenticate` tool a server waiting on a login offers
//! (v0.1.22).
//! Taken from `../kimi-code-main`. A server that answers 401 has no tools to
//! the servers are simply missing, and the model discovers the gap by planning
//! around tools that are not there. What it needs instead is one thing it can
//! call — a tool per waiting server, named `mcp__<server>__authenticate`, whose
//! whole job is to run the login and bring the real tools back inside the same
//! turn.
//! The tool exists only where a login could actually succeed: an HTTP server,
//! OAuth not switched off, a credential file to write to. Where it cannot, the
//! server contributes nothing at all, because a tool that can only ever answer
//! "this is impossible" costs a turn to learn what the description already said.

use std::sync::Arc;

use notagent_agent::types::{
    AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::types::{TextContent, TextOrImageContent};
use notagent_tui::tui::ComponentRef;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::core::mcp::ServerName;
use crate::core::mcp::auth::McpAuthPrompt;
use crate::core::mcp::manager::McpManager;
use crate::core::tools::mcp_render::format_mcp_result;
use crate::core::tools::render_utils::call_title;
use crate::core::tools::tool_definition::{
    ToolContext, ToolDefinition, ToolRenderContext, ToolRenderResult, ToolRenderResultOptions,
    render_text_call, render_text_result,
};
use crate::modes::interactive::theme::theme::Theme;

/// Run after a login, to put the server's real tools into the registry.
/// The session owns that rebuild; the tool only says when it is due.
pub type McpToolsChanged = Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;

/// The name the login tool takes on a server.
pub fn auth_tool_name(server: &ServerName) -> String {
    crate::core::mcp::manager::qualify_tool_name(server, "authenticate")
}

pub struct McpAuthToolDefinition {
    server: ServerName,
    name: String,
    description: String,
    manager: Arc<McpManager>,
    tools_changed: Option<McpToolsChanged>,
    parameters: Value,
}

pub fn create_mcp_auth_tool_definition(
    server: ServerName,
    manager: Arc<McpManager>,
    tools_changed: Option<McpToolsChanged>,
) -> McpAuthToolDefinition {
    let name = auth_tool_name(&server);
    let description = format!(
        "Sign in to the MCP server `{server}`, which is refusing every request until you do. \
         Running this opens a browser window for the user and waits for them to finish; the \
         server's own tools become available immediately afterwards. Takes no arguments. Call \
         it when you need something from `{server}` — no other tool of that server can be used \
         first."
    );
    McpAuthToolDefinition {
        server,
        name,
        description,
        manager,
        tools_changed,
        parameters: serde_json::json!({ "type": "object", "properties": {} }),
    }
}

impl ToolDefinition for McpAuthToolDefinition {
    fn name(&self) -> &str {
        &self.name
    }

    fn label(&self) -> &str {
        "authenticate"
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn constrained_sampling(&self) -> Option<&notagent_ai::types::ConstrainedSampling> {
        None
    }

    fn render_call(
        &self,
        _args: &Value,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        let title = call_title(theme, self.server.as_str());
        Some(render_text_call(context, &format!("{title}authenticate")))
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
        _params: Value,
        signal: Option<CancellationToken>,
        on_update: Option<AgentToolUpdateCallback>,
        _context: Option<ToolContext>,
    ) -> BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>> {
        Box::pin(async move {
            if matches!(&signal, Some(signal) if signal.is_cancelled()) {
                return Err(ToolExecutionError::new("Operation aborted"));
            }

            // The URL is streamed the moment it exists, before the wait: a
            // browser that did not come up leaves the user with nothing to act
            // on otherwise, and the wait is minutes long.
            let announce = |prompt: McpAuthPrompt| {
                let Some(on_update) = &on_update else {
                    return;
                };
                on_update(result_text(format!(
                    "Waiting for the sign-in to `{}` to finish in the browser.\n{}",
                    self.server, prompt.authorization_url
                )));
            };

            let login = self.manager.authenticate(&self.server, announce);
            // A cancelled turn must not leave the login holding its port and
            // its browser tab for the rest of the five-minute window.
            let count = match signal {
                Some(signal) => tokio::select! {
                    outcome = login => outcome,
                    () = signal.cancelled() => return Err(ToolExecutionError::new("Operation aborted")),
                },
                None => login.await,
            }
            .map_err(|error| {
                ToolExecutionError::new(format!(
                    "signing in to `{}` failed: {error}",
                    self.server
                ))
            })?;

            if let Some(tools_changed) = &self.tools_changed {
                tools_changed().await;
            }

            Ok(result_text(format!(
                "Signed in to `{}`. Its {count} tool{} {} available now; \
                 call the one you need.",
                self.server,
                if count == 1 { "" } else { "s" },
                if count == 1 { "is" } else { "are" },
            )))
        })
    }
}

fn result_text(text: String) -> AgentToolResult {
    AgentToolResult {
        content: vec![TextOrImageContent::Text(TextContent::new(text))],
        details: None,
        usage: None,
        added_tool_names: None,
        terminate: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mcp::{McpHttpServer, McpOAuthSetting, McpServerConfig, McpStdioServer};
    use std::collections::BTreeMap;

    fn http(url: &str, oauth: McpOAuthSetting) -> McpServerConfig {
        McpServerConfig::Http(McpHttpServer {
            url: url.to_owned(),
            oauth,
            ..McpHttpServer::default()
        })
    }

    fn manager_with(config: McpServerConfig, credentials: Option<&std::path::Path>) -> McpManager {
        let mut configs = BTreeMap::new();
        configs.insert(ServerName::from("remote"), config);
        let manager = McpManager::new(configs, BTreeMap::new());
        match credentials {
            Some(path) => manager.with_credentials(path),
            None => manager,
        }
    }

    #[tokio::test]
    async fn a_local_process_offers_no_login() {
        let manager = manager_with(
            McpServerConfig::Stdio(McpStdioServer {
                command: "true".to_owned(),
                ..McpStdioServer::default()
            }),
            Some(std::path::Path::new("/tmp/does-not-matter.json")),
        );

        assert!(!manager.can_authenticate(&ServerName::from("remote")).await);
    }

    #[tokio::test]
    async fn a_server_with_oauth_switched_off_offers_no_login() {
        let manager = manager_with(
            http("https://example.test/mcp", McpOAuthSetting::Disabled),
            Some(std::path::Path::new("/tmp/does-not-matter.json")),
        );

        assert!(!manager.can_authenticate(&ServerName::from("remote")).await);
    }

    #[tokio::test]
    async fn a_session_without_a_credential_file_offers_no_login() {
        let manager = manager_with(
            http("https://example.test/mcp", McpOAuthSetting::AutoDetect),
            None,
        );

        assert!(!manager.can_authenticate(&ServerName::from("remote")).await);
    }

    #[tokio::test]
    async fn a_remote_server_that_could_be_logged_into_offers_the_tool() {
        let manager = manager_with(
            http("https://example.test/mcp", McpOAuthSetting::AutoDetect),
            Some(std::path::Path::new("/tmp/does-not-matter.json")),
        );

        assert!(manager.can_authenticate(&ServerName::from("remote")).await);
    }

    #[tokio::test]
    async fn a_login_that_cannot_be_run_says_so_instead_of_hanging() {
        let manager = Arc::new(manager_with(
            McpServerConfig::Stdio(McpStdioServer {
                command: "true".to_owned(),
                ..McpStdioServer::default()
            }),
            Some(std::path::Path::new("/tmp/does-not-matter.json")),
        ));
        let tool = create_mcp_auth_tool_definition(ServerName::from("remote"), manager, None);

        let error = tool
            .execute("id", Value::Null, None, None, None)
            .await
            .expect_err("a local process has no login");

        assert!(error.to_string().contains("no login to perform"), "{error}");
    }

    #[tokio::test]
    async fn an_already_cancelled_turn_starts_no_browser() {
        let manager = Arc::new(manager_with(
            http("https://example.test/mcp", McpOAuthSetting::AutoDetect),
            Some(std::path::Path::new("/tmp/does-not-matter.json")),
        ));
        let tool = create_mcp_auth_tool_definition(ServerName::from("remote"), manager, None);
        let signal = CancellationToken::new();
        signal.cancel();

        let error = tool
            .execute("id", Value::Null, Some(signal), None, None)
            .await
            .expect_err("a cancelled turn runs nothing");

        assert!(error.to_string().contains("aborted"), "{error}");
    }

    #[test]
    fn the_tool_is_named_after_its_server() {
        assert_eq!(
            auth_tool_name(&ServerName::from("github")),
            "mcp__github__authenticate"
        );
    }

    #[test]
    fn the_description_names_the_server_it_signs_into() {
        let tool = create_mcp_auth_tool_definition(
            ServerName::from("github"),
            Arc::new(McpManager::empty()),
            None,
        );

        assert!(
            tool.description().contains("`github`"),
            "{}",
            tool.description()
        );
        assert_eq!(tool.parameters()["type"], "object");
    }
}
