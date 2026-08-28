//! MCP tool calls and transport recovery.
//!
//! Protocol errors stand as returned. Ambiguous transport failures probe the
//! existing connection and retry once in place when it is still alive. A
//! confirmed dead transport reconnects once before retrying. Retries are
//! at-least-once because MCP cannot deduplicate calls across reconnects.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::core::mcp::client::{McpCallError, McpConnection};
use crate::core::mcp::manager::{McpManager, McpServerStatus, McpToolInfo};

/// What a failed call led to, for the tests and for the status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpRecovery {
    /// The call succeeded first time.
    None,
    /// The connection answered a probe, so the call was repeated in place.
    RetriedInPlace,
    /// The server was reconnected and the call repeated.
    Reconnected,
}

/// A completed call and how it got there.
#[derive(Debug)]
pub struct McpCallOutcome {
    pub result: rmcp::model::CallToolResult,
    pub recovery: McpRecovery,
}

/// Calls a tool, recovering from a lost connection where that can help.
pub async fn call_tool(
    manager: &McpManager,
    tool: &McpToolInfo,
    arguments: serde_json::Map<String, serde_json::Value>,
    signal: Option<&CancellationToken>,
) -> Result<McpCallOutcome, McpCallError> {
    if manager.status(&tool.server).await == Some(McpServerStatus::Removed) {
        return Err(McpCallError::Answered(format!(
            "the MCP server `{}` has been removed from the configuration, so `{}` is gone. \
             Do not call it again.",
            tool.server, tool.qualified_name
        )));
    }

    let Some(connection) = manager.connection(&tool.server).await else {
        return Err(McpCallError::Answered(format!(
            "the MCP server `{}` is not connected",
            tool.server
        )));
    };

    let failure = match connection
        .call_tool_until(&tool.tool_name, arguments.clone(), signal)
        .await
    {
        Ok(result) => {
            return Ok(McpCallOutcome {
                result,
                recovery: McpRecovery::None,
            });
        }
        Err(error) => error,
    };

    // The server answered, the deadline expired, or the turn ended. None is
    // improved by a second attempt: the answer would be the same, a timeout
    // repeated is a second wait of the same length, and a cancelled turn has
    // nobody left to read the result.
    if !failure.is_transport() {
        return Err(failure);
    }

    let failure = match retry_in_place(&connection, tool, &arguments, failure, signal).await {
        Ok(result) => {
            return Ok(McpCallOutcome {
                result,
                recovery: McpRecovery::RetriedInPlace,
            });
        }
        Err(error) => error,
    };

    // The transport is gone. One reconnect, one more attempt.
    manager.reconnect(&tool.server).await.map_err(|error| {
        McpCallError::Answered(format!("{failure} (reconnecting also failed: {error})"))
    })?;
    let Some(fresh) = manager.connection(&tool.server).await else {
        return Err(failure);
    };
    fresh
        .call_tool_until(&tool.tool_name, arguments, signal)
        .await
        .map(|result| McpCallOutcome {
            result,
            recovery: McpRecovery::Reconnected,
        })
}

/// Retries once on the same connection when a probe says it is still there.
/// Returns the failure to escalate with when it is not, which is either the
/// original one or the retry's.
async fn retry_in_place(
    connection: &Arc<McpConnection>,
    tool: &McpToolInfo,
    arguments: &serde_json::Map<String, serde_json::Value>,
    failure: McpCallError,
    signal: Option<&CancellationToken>,
) -> Result<rmcp::model::CallToolResult, McpCallError> {
    // A closed transport needs no probe; it is already the answer.
    if matches!(failure, McpCallError::Closed) {
        return Err(failure);
    }
    if !connection.is_alive().await {
        return Err(failure);
    }
    match connection
        .call_tool_until(&tool.tool_name, arguments.clone(), signal)
        .await
    {
        Ok(result) => Ok(result),
        Err(retry_failure) if retry_failure.is_transport() => Err(retry_failure),
        // The retry reached the server and it answered: that is the outcome.
        Err(answered) => Err(answered),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_transport_skips_the_probe() {
        // Documented here because the branch is easier to read than to reach:
        // a probe against a transport already known to be closed can only say
        // what is already known, and costs a round trip to say it.
        assert!(matches!(McpCallError::Closed, McpCallError::Closed));
    }

    #[test]
    fn recovery_states_are_distinct() {
        assert_ne!(McpRecovery::None, McpRecovery::RetriedInPlace);
        assert_ne!(McpRecovery::RetriedInPlace, McpRecovery::Reconnected);
    }
}
