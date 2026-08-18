//! The MCP client against a real server (port addition, v0.1.22).
//!
//! Every case here spawns `mcp_test_server` as a separate process and speaks
//! the protocol to it. The server is written to misbehave on demand, so the
//! recovery paths are exercised rather than asserted: a handshake that never
//! answers, a call that never returns, a server that dies mid-call, one that
//! answers with a JSON-RPC error.
//!
//! Mocking these would test what this port believes about MCP, which is the
//! thing under test.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use notagent::core::mcp::client::{McpCallError, McpConnection};
use notagent::core::mcp::{McpServerConfig, McpStdioServer};

/// A server configured to behave in one particular way.
fn server(behaviour: &str, timeout_secs: u64) -> McpServerConfig {
    McpServerConfig::Stdio(McpStdioServer {
        command: env!("CARGO_BIN_EXE_mcp_test_server").to_owned(),
        args: vec![behaviour.to_owned()],
        timeout: Some(timeout_secs),
        ..McpStdioServer::default()
    })
}

async fn connect(behaviour: &str, timeout_secs: u64) -> Result<McpConnection, McpCallError> {
    McpConnection::connect(&server(behaviour, timeout_secs), &BTreeMap::new()).await
}

fn arguments(text: &str) -> serde_json::Map<String, serde_json::Value> {
    let mut map = serde_json::Map::new();
    map.insert(
        "text".to_owned(),
        serde_json::Value::String(text.to_owned()),
    );
    map
}

fn text_of(result: &rmcp::model::CallToolResult) -> String {
    result
        .content
        .iter()
        .flat_map(|content| content.as_text())
        .map(|text| text.text.clone())
        .collect::<Vec<_>>()
        .join("")
}

// ---- the happy path --------------------------------------------------------

#[tokio::test]
async fn a_real_server_completes_handshake_discovery_and_a_call() {
    let connection = connect("normal", 10).await.expect("connects");

    let tools = connection.list_tools().await.expect("lists");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");

    let result = connection
        .call_tool("echo", arguments("round trip"))
        .await
        .expect("calls");

    assert_eq!(text_of(&result), "round trip");
    connection.close().await;
}

#[tokio::test]
async fn two_calls_on_one_connection_run_concurrently() {
    let connection = connect("normal", 10).await.expect("connects");

    let (first, second) = tokio::join!(
        connection.call_tool("echo", arguments("one")),
        connection.call_tool("echo", arguments("two")),
    );

    assert_eq!(text_of(&first.expect("first call")), "one");
    assert_eq!(text_of(&second.expect("second call")), "two");
    connection.close().await;
}

// ---- deadlines -------------------------------------------------------------

#[tokio::test]
async fn a_handshake_that_never_answers_expires_instead_of_hanging() {
    let started = Instant::now();
    let error = connect("hang-handshake", 1)
        .await
        .expect_err("the handshake never completes");

    assert!(matches!(error, McpCallError::TimedOut(_)), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "took {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_call_that_never_answers_expires_and_leaves_the_connection_usable() {
    let connection = connect("hang-call", 1).await.expect("connects");
    let started = Instant::now();

    let error = connection
        .call_tool("echo", arguments("never returns"))
        .await
        .expect_err("the call never completes");

    assert!(matches!(error, McpCallError::TimedOut(_)), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "took {:?}",
        started.elapsed()
    );
    connection.close().await;
}

#[tokio::test]
async fn the_configured_timeout_is_the_one_that_applies() {
    // Two seconds is far below the 300-second default; if the default were
    // still in force this test would run for five minutes.
    let started = Instant::now();
    let _ = connect("hang-handshake", 2).await;
    let elapsed = started.elapsed();

    assert!(elapsed >= Duration::from_secs(2), "took {elapsed:?}");
    assert!(elapsed < Duration::from_secs(10), "took {elapsed:?}");
}

// ---- failure shapes --------------------------------------------------------

#[tokio::test]
async fn a_server_error_is_not_mistaken_for_a_transport_failure() {
    let connection = connect("error-on-call", 10).await.expect("connects");

    let error = connection
        .call_tool("echo", arguments("rejected"))
        .await
        .expect_err("the server refuses");

    assert!(
        !error.is_transport(),
        "reconnecting cannot change a server's answer: {error}"
    );
    connection.close().await;
}

#[tokio::test]
async fn a_server_that_dies_mid_call_is_reported_as_a_transport_failure() {
    let connection = connect("die-on-call", 10).await.expect("connects");

    let error = connection
        .call_tool("echo", arguments("goodbye"))
        .await
        .expect_err("the server exits");

    assert!(
        error.is_transport(),
        "a dead server is worth reconnecting: {error}"
    );
}

#[tokio::test]
async fn a_closed_transport_is_visible_to_the_liveness_probe() {
    let connection = connect("close-after-call", 10).await.expect("connects");

    connection
        .call_tool("echo", arguments("last words"))
        .await
        .expect("the first call is answered");
    // The server closed its output after answering; the probe is what tells a
    // blip from a death.
    let alive = tokio::time::timeout(Duration::from_secs(5), connection.is_alive())
        .await
        .expect("the probe is bounded");

    assert!(!alive);
}

#[tokio::test]
async fn a_server_that_never_reads_its_input_does_not_block_the_client() {
    // Its stderr fills the pipe; the client's drain is what keeps this from
    // deadlocking. Without the drain this call would never return.
    let started = Instant::now();
    let error = connect("noisy-stderr", 2)
        .await
        .expect_err("it never answers the handshake");

    assert!(matches!(error, McpCallError::TimedOut(_)), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "took {:?}",
        started.elapsed()
    );
}

// ---- what a server may send ------------------------------------------------

#[tokio::test]
async fn one_malformed_tool_costs_its_server_the_whole_list_and_says_so() {
    // Found by this test rather than assumed: the SDK deserializes a tool list
    // whole, so a single tool whose input schema is not an object takes the
    // server's other tools with it. That is the server being broken; what
    // matters is that it is reported as such and that the failure stays inside
    // this one server.
    let connection = connect("invalid-schema", 10).await.expect("connects");

    let error = connection
        .list_tools()
        .await
        .expect_err("the list cannot be read");

    assert!(!error.is_transport(), "{error}");
    assert!(error.to_string().contains("malformed"), "{error}");
    connection.close().await;
}

#[tokio::test]
async fn a_megabyte_of_output_arrives_whole() {
    let connection = connect("huge-result", 20).await.expect("connects");

    let result = connection
        .call_tool("echo", arguments("ignored"))
        .await
        .expect("calls");

    assert_eq!(text_of(&result).len(), 1024 * 1024);
    connection.close().await;
}

// ---- process lifetime ------------------------------------------------------

#[tokio::test]
async fn closing_a_connection_ends_its_child() {
    // Its own behaviour string, so the process check below sees this case's
    // server and no other case's. The server treats anything it does not know
    // as `normal`.
    let connection = connect("lifetime-close", 10).await.expect("connects");
    connection
        .call_tool("echo", arguments("alive"))
        .await
        .expect("calls");
    assert_eq!(running_test_servers("lifetime-close"), 1);

    connection.close().await;

    assert_eq!(wait_for_no_servers("lifetime-close").await, 0);
}

#[tokio::test]
async fn dropping_a_connection_ends_its_child() {
    {
        let connection = connect("lifetime-drop", 10).await.expect("connects");
        connection
            .call_tool("echo", arguments("alive"))
            .await
            .expect("calls");
        assert_eq!(running_test_servers("lifetime-drop"), 1);
    }

    assert_eq!(wait_for_no_servers("lifetime-drop").await, 0);
}

/// How many test servers with this behaviour are running.
fn running_test_servers(behaviour: &str) -> usize {
    let output = std::process::Command::new("pgrep")
        .arg("-f")
        .arg(format!("mcp_test_server {behaviour}"))
        .output();
    match output {
        Ok(output) => String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count(),
        // Without pgrep the check cannot run.
        Err(_) => 0,
    }
}

/// Waits briefly for the children to go, so the assertion is about them being
/// reaped rather than about how fast the operating system got round to it.
async fn wait_for_no_servers(behaviour: &str) -> usize {
    for _ in 0..40 {
        let running = running_test_servers(behaviour);
        if running == 0 {
            return 0;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    running_test_servers(behaviour)
}

// ---- the manager and the failure triage ------------------------------------
//
// The triage is the one behaviour where the two reference implementations
// disagree, so each of its three branches is exercised against a real server
// and the attempt journal is what proves how many calls actually went out.

use notagent::core::mcp::ServerName;
use notagent::core::mcp::call::{McpRecovery, call_tool};
use notagent::core::mcp::manager::{McpManager, McpServerStatus, McpToolInfo};
use std::sync::Arc;

/// A manager with one server, journalling every call it receives.
fn manager_with(behaviour: &str, journal: &std::path::Path, timeout_secs: u64) -> Arc<McpManager> {
    let config = McpServerConfig::Stdio(McpStdioServer {
        command: env!("CARGO_BIN_EXE_mcp_test_server").to_owned(),
        args: vec![behaviour.to_owned(), journal.to_string_lossy().into_owned()],
        timeout: Some(timeout_secs),
        ..McpStdioServer::default()
    });
    Arc::new(McpManager::new(
        std::collections::BTreeMap::from([(ServerName::from("s"), config)]),
        BTreeMap::new(),
    ))
}

fn journal_path(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("notagent-mcp-journal-{name}.log"));
    let _ = std::fs::remove_file(&path);
    path
}

fn attempts(path: &std::path::Path) -> usize {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
}

fn echo_tool(tools: &[McpToolInfo]) -> McpToolInfo {
    tools
        .iter()
        .find(|tool| tool.tool_name == "echo")
        .cloned()
        .expect("the server offers echo")
}

#[tokio::test]
async fn a_working_call_goes_out_once() {
    let journal = journal_path("normal-once");
    let manager = manager_with("normal", &journal, 10);
    let tools = manager.ensure_connected(&ServerName::from("s")).await;

    let outcome = call_tool(&manager, &echo_tool(&tools), arguments("hello"), None)
        .await
        .expect("calls");

    assert_eq!(outcome.recovery, McpRecovery::None);
    assert_eq!(attempts(&journal), 1);
    manager.shutdown().await;
}

#[tokio::test]
async fn a_server_error_is_not_retried() {
    // The reference would try this five times. The server already answered;
    // asking again can only produce the same answer.
    let journal = journal_path("error-not-retried");
    let manager = manager_with("error-on-call", &journal, 10);
    let tools = manager.ensure_connected(&ServerName::from("s")).await;

    let error = call_tool(&manager, &echo_tool(&tools), arguments("rejected"), None)
        .await
        .expect_err("the server refuses");

    assert!(!error.is_transport(), "{error}");
    assert_eq!(attempts(&journal), 1, "a refusal was asked twice");
    manager.shutdown().await;
}

#[tokio::test]
async fn a_dead_transport_is_reconnected_and_the_call_repeated() {
    // The reference never reconnects, so this call would simply fail there.
    let journal = journal_path("die-once");
    let manager = manager_with("die-once", &journal, 10);
    let tools = manager.ensure_connected(&ServerName::from("s")).await;

    let outcome = call_tool(&manager, &echo_tool(&tools), arguments("survives"), None)
        .await
        .expect("the second attempt lands on a fresh connection");

    assert_eq!(outcome.recovery, McpRecovery::Reconnected);
    assert_eq!(text_of(&outcome.result), "survives");
    // Twice: the attempt that killed the server, and the one after the
    // reconnect. At-least-once is the documented trade.
    assert_eq!(attempts(&journal), 2);
    manager.shutdown().await;
}

#[tokio::test]
async fn a_timed_out_call_is_not_repeated() {
    // Repeating a timeout is a second wait of the same length for the same
    // outcome.
    let journal = journal_path("hang-not-retried");
    let manager = manager_with("hang-call", &journal, 1);
    let tools = manager.ensure_connected(&ServerName::from("s")).await;

    let error = call_tool(&manager, &echo_tool(&tools), arguments("never"), None)
        .await
        .expect_err("it never answers");

    assert!(matches!(error, McpCallError::TimedOut(_)), "{error}");
    assert_eq!(attempts(&journal), 1);
    manager.shutdown().await;
}

// ---- what one broken server costs the others -------------------------------

#[tokio::test]
async fn a_failing_server_leaves_the_others_working() {
    let good = McpServerConfig::Stdio(McpStdioServer {
        command: env!("CARGO_BIN_EXE_mcp_test_server").to_owned(),
        args: vec!["normal".to_owned()],
        timeout: Some(10),
        ..McpStdioServer::default()
    });
    let hung = McpServerConfig::Stdio(McpStdioServer {
        command: env!("CARGO_BIN_EXE_mcp_test_server").to_owned(),
        args: vec!["hang-handshake".to_owned()],
        timeout: Some(1),
        ..McpStdioServer::default()
    });
    let manager = McpManager::new(
        std::collections::BTreeMap::from([
            (ServerName::from("good"), good),
            (ServerName::from("hung"), hung),
        ]),
        BTreeMap::new(),
    );

    let tools = manager.ensure_all_connected().await;
    let entries = manager.entries().await;

    assert_eq!(tools.len(), 1, "the working server still contributes");
    assert_eq!(tools[0].qualified_name, "mcp__good__echo");
    let status = |name: &str| {
        entries
            .iter()
            .find(|entry| entry.name.as_str() == name)
            .map(|entry| entry.status)
    };
    assert_eq!(status("good"), Some(McpServerStatus::Connected));
    assert_eq!(status("hung"), Some(McpServerStatus::Failed));
    manager.shutdown().await;
}

#[tokio::test]
async fn a_removed_server_tells_the_model_to_stop_calling_it() {
    let journal = journal_path("removed");
    let manager = manager_with("normal", &journal, 10);
    let name = ServerName::from("s");
    let tools = manager.ensure_connected(&name).await;

    manager.mark_removed(&name).await;
    let error = call_tool(&manager, &echo_tool(&tools), arguments("gone"), None)
        .await
        .expect_err("the server is gone");

    assert!(error.to_string().contains("removed"), "{error}");
    assert!(
        error.to_string().contains("Do not call it again"),
        "{error}"
    );
    assert_eq!(manager.status(&name).await, Some(McpServerStatus::Removed));
}

#[tokio::test]
async fn a_reconnect_brings_a_failed_server_back() {
    let journal = journal_path("reconnect");
    let manager = manager_with("normal", &journal, 10);
    let name = ServerName::from("s");
    manager.ensure_connected(&name).await;
    manager.shutdown().await;

    let tool_count = manager.reconnect(&name).await.expect("reconnects");

    assert_eq!(tool_count, 1);
    assert_eq!(
        manager.status(&name).await,
        Some(McpServerStatus::Connected)
    );
    manager.shutdown().await;
}

#[tokio::test]
async fn a_manifest_larger_than_the_ceiling_is_cut_to_it() {
    let journal = journal_path("many-tools");
    let manager = manager_with("many-tools", &journal, 10);

    let tools = manager.ensure_connected(&ServerName::from("s")).await;

    assert_eq!(
        tools.len(),
        notagent::core::mcp::manager::MAX_TOOLS_PER_SERVER
    );
    manager.shutdown().await;
}

#[tokio::test]
async fn shutdown_ends_every_child() {
    let journal = journal_path("shutdown");
    let manager = manager_with("lifetime-shutdown", &journal, 10);
    manager.ensure_connected(&ServerName::from("s")).await;
    assert_eq!(running_test_servers("lifetime-shutdown"), 1);

    manager.shutdown().await;

    assert_eq!(wait_for_no_servers("lifetime-shutdown").await, 0);
}

#[tokio::test]
async fn a_cancelled_call_returns_at_once_instead_of_waiting_out_the_deadline() {
    // The deadline is far longer than the test; if cancellation did not reach
    // the call, this would sit here for half a minute.
    let connection = connect("hang-call", 30).await.expect("connects");
    let signal = tokio_util::sync::CancellationToken::new();
    let cancelling = signal.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancelling.cancel();
    });

    let started = Instant::now();
    let error = connection
        .call_tool_until("echo", arguments("never returns"), Some(&signal))
        .await
        .expect_err("the turn ended");

    assert!(matches!(error, McpCallError::Cancelled), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "waited {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_cancelled_call_is_not_retried_or_reconnected() {
    let journal = journal_path("cancelled");
    let manager = manager_with("hang-call-retry", &journal, 30);
    let name = ServerName::from("s");
    let tools = manager.ensure_connected(&name).await;
    let signal = tokio_util::sync::CancellationToken::new();
    let cancelling = signal.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancelling.cancel();
    });

    let error = notagent::core::mcp::call::call_tool(
        &manager,
        &echo_tool(&tools),
        arguments("never returns"),
        Some(&signal),
    )
    .await
    .expect_err("the turn ended");

    assert!(matches!(error, McpCallError::Cancelled), "{error}");
    // A cancelled turn has nobody left to read a second answer, and a retry
    // would run the server's side effect again for nothing.
    assert_eq!(attempts(&journal), 1);
    manager.shutdown().await;
}

#[tokio::test]
async fn a_cancelled_call_leaves_no_child_behind() {
    let journal = journal_path("cancelled-child");
    let manager = manager_with("hang-call-lifetime", &journal, 30);
    let name = ServerName::from("s");
    let tools = manager.ensure_connected(&name).await;
    assert_eq!(running_test_servers("hang-call-lifetime"), 1);

    let signal = tokio_util::sync::CancellationToken::new();
    let cancelling = signal.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancelling.cancel();
    });
    let _ = notagent::core::mcp::call::call_tool(
        &manager,
        &echo_tool(&tools),
        arguments("never returns"),
        Some(&signal),
    )
    .await;

    manager.shutdown().await;

    assert_eq!(wait_for_no_servers("hang-call-lifetime").await, 0);
}
