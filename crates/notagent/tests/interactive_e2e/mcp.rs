//! `/mcp` (port addition, v0.1.22) — the servers, a reconnect and a login.
//!
//! The protocol itself is covered against a real server in `tests/mcp_client.rs`
//! and the OAuth path against a real HTTP server in `tests/mcp_auth.rs`. What
//! runs here is the path a user takes: the typed command reaches the handler,
//! and every branch of it answers on screen rather than leaving the session
//! looking as if nothing happened.
//!
//! The session under test has no `.mcp.json`, so this pins the empty case and
//! the argument handling. A server that exists is the other suites' subject.

use notagent::config::APP_NAME;

use super::harness::{InteractiveE2e, run_local};

#[tokio::test(flavor = "current_thread")]
async fn the_mcp_command_reports_its_servers_and_every_argument() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        // Nothing configured: the notice says where a configuration would go,
        // because "no servers" on its own is a dead end.
        driver.submit("/mcp").await;
        driver.wait_for("No MCP servers configured").await;
        driver.wait_for(".mcp.json").await;

        // An argument that is not a subcommand names the ones that are.
        driver.submit("/mcp sometimes").await;
        driver.wait_for("Unknown /mcp argument: sometimes").await;
        // Matched in halves: the notice wraps at the pane's width, and a match
        // that spans the wrap point would be pinning the width, not the text.
        driver.wait_for("reconnect <server>").await;
        driver.wait_for("or `login").await;

        // Each subcommand without a server says what it wanted.
        driver.submit("/mcp reconnect").await;
        driver.wait_for("Usage: /mcp reconnect <server>").await;

        driver.submit("/mcp login").await;
        driver.wait_for("Usage: /mcp login <server>").await;

        // A server nobody configured is reported rather than silently retried,
        // and the login for one never reaches a browser.
        driver.submit("/mcp reconnect nowhere").await;
        driver
            .wait_for("MCP `nowhere`: no server named `nowhere` is configured")
            .await;

        driver.submit("/mcp login nowhere").await;
        driver
            .wait_for("MCP `nowhere`: no server named `nowhere` is configured")
            .await;
    })
    .await;
}
