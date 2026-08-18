//! Lending our tools out, over the wire (port addition, v0.1.22).
//!
//! Driven with this port's own MCP client against the endpoint, because that is
//! the only way to find out whether an external agent could actually use it: a
//! handler tested by calling its methods directly would pass with the transport,
//! the router and the token check all broken.
//!
//! One test, five scenarios, on purpose. The endpoint is process-wide and its
//! server runs on whichever runtime first started it; a second `#[tokio::test]`
//! brings its own runtime, and the first one to finish takes the server down
//! while the static URL still points at it. The application has one runtime for
//! its whole life, so this is a property of the test harness rather than of the
//! endpoint — but splitting these into separate tests makes them fail for a
//! reason that has nothing to do with what they check.

use std::collections::BTreeMap;
use std::sync::Arc;

use notagent::core::mcp::client::McpConnection;
use notagent::core::mcp::lend::{LentToolSource, lend_tools};
use notagent::core::mcp::{McpHttpServer, McpServerConfig};
use notagent::core::tools::tool_definition::ToolDefinition;

/// One tool that answers with what it was given.
struct Echo {
    parameters: serde_json::Value,
}

impl ToolDefinition for Echo {
    fn name(&self) -> &str {
        "echo"
    }
    fn label(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "Repeats its argument."
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.parameters
    }
    fn constrained_sampling(&self) -> Option<&notagent_ai::types::ConstrainedSampling> {
        None
    }
    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        params: serde_json::Value,
        _signal: Option<tokio_util::sync::CancellationToken>,
        _on_update: Option<notagent_agent::types::AgentToolUpdateCallback>,
        _context: Option<notagent::core::tools::tool_definition::ToolContext>,
    ) -> notagent_agent::types::BoxFuture<
        'a,
        Result<notagent_agent::types::AgentToolResult, notagent_agent::types::ToolExecutionError>,
    > {
        Box::pin(async move {
            let said = params
                .get("say")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("nothing");
            if said == "fail" {
                return Err(notagent_agent::types::ToolExecutionError::new(
                    "the tool refused",
                ));
            }
            Ok(notagent_agent::types::AgentToolResult {
                content: vec![notagent_ai::types::TextOrImageContent::Text(
                    notagent_ai::types::TextContent::new(format!("you said {said}")),
                )],
                details: None,
                usage: None,
                added_tool_names: None,
                terminate: None,
            })
        })
    }
}

struct OneTool;

impl LentToolSource for OneTool {
    fn lendable(&self) -> Vec<Arc<dyn ToolDefinition>> {
        vec![Arc::new(Echo {
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "say": { "type": "string" } },
            }),
        })]
    }
}

fn client_config(url: &str, authorization: Option<&str>) -> McpServerConfig {
    let mut headers = BTreeMap::new();
    if let Some(value) = authorization {
        headers.insert("Authorization".to_owned(), value.to_owned());
    }
    McpServerConfig::Http(McpHttpServer {
        url: url.to_owned(),
        headers,
        timeout: Some(10),
        startup_timeout: Some(10),
        ..McpHttpServer::default()
    })
}

#[tokio::test]
async fn the_endpoint_serves_what_it_lends_and_nothing_else() {
    let endpoint = lend_tools(Arc::new(OneTool)).await.expect("lends");
    let authorized = client_config(&endpoint.url, Some(&endpoint.authorization));

    // Without the token there is no way in. Loopback is not a boundary — every
    // process on this machine can reach it.
    let refused = McpConnection::connect(&client_config(&endpoint.url, None), &BTreeMap::new())
        .await
        .expect_err("no token, no entry");
    assert!(
        notagent::core::mcp::client::is_unauthorized_text(&refused.to_string()),
        "{refused}"
    );

    let connection = McpConnection::connect(&authorized, &BTreeMap::new())
        .await
        .expect("connects to the endpoint");

    // What was lent is listed, with the schema an agent needs to call it.
    let tools = connection.list_tools().await.expect("lists");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name.as_ref(), "echo");
    assert!(tools[0].input_schema.contains_key("properties"));

    // And it runs.
    let mut arguments = serde_json::Map::new();
    arguments.insert("say".to_owned(), serde_json::json!("hello"));
    let result = connection
        .call_tool("echo", arguments)
        .await
        .expect("calls");
    let text = result
        .content
        .iter()
        .filter_map(|block| block.as_text().map(|text| text.text.clone()))
        .collect::<Vec<_>>()
        .join("");
    assert_eq!(text, "you said hello");
    assert_ne!(result.is_error, Some(true));

    // A tool that was never offered is refused rather than attempted.
    let unknown = connection
        .call_tool("bash", serde_json::Map::new())
        .await
        .expect_err("that tool was never offered");
    assert!(unknown.to_string().contains("not available"), "{unknown}");

    // A tool that fails answers inside the result: the borrowing agent should
    // read the message and adapt, not lose its connection over it.
    let mut failing = serde_json::Map::new();
    failing.insert("say".to_owned(), serde_json::json!("fail"));
    let failed = connection
        .call_tool("echo", failing)
        .await
        .expect("answers");
    assert_eq!(failed.is_error, Some(true));

    // The connection survives that.
    let mut again = serde_json::Map::new();
    again.insert("say".to_owned(), serde_json::json!("again"));
    assert!(connection.call_tool("echo", again).await.is_ok());

    // The token stops meaning anything once the session that issued it is over.
    drop(endpoint);
    let revoked = McpConnection::connect(&authorized, &BTreeMap::new())
        .await
        .expect_err("the token was revoked");
    assert!(
        notagent::core::mcp::client::is_unauthorized_text(&revoked.to_string()),
        "{revoked}"
    );
}
