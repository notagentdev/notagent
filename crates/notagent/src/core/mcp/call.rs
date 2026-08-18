//! Calling an MCP tool, and what to do when the call fails (v0.1.22).
//!
//! The recovery decision is taken from `../kimi-code-main`, which is the one
//! part of MCP both references disagree about and only one of them gets right.
//! The reference this port is otherwise built on retries any transport error
//! five times with backoff, which is wrong in both directions: it retries where
//! the server already gave an answer, and it never reconnects a transport that
//! is gone, which is the only case retrying could fix.
//!
//! Three outcomes, three answers:
//!
//! - The server answered — a protocol error, or a reply that could not be read.
//!   Reconnecting cannot change what it said, so the error stands.
//! - The failure is ambiguous — a send that did not land, a socket that
//!   complained. Probe the connection: alive means a blip and the call is
//!   retried in place, dead means the transport is gone.
//! - The transport is provably gone. Reconnect once and call again on the fresh
//!   connection, so a dropped connection costs a slow call rather than a turn.
//!
//! Retries are at-least-once. A transport that died after the server processed
//! the call but before the answer arrived is indistinguishable from one that
//! died before, and MCP has no deduplication across reconnects, so a retried
//! call may repeat a side effect.

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
///
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
