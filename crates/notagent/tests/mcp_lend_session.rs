//! A borrowed call against a real session (port addition, v0.1.22).
//!
//! `tests/mcp_lend.rs` proves the endpoint: what it lists, what it refuses, what
//! a token means. This proves the half that matters more — that a call arriving
//! over that endpoint meets the session's permission chain.
//!
//! It has to be a separate binary rather than another case in that file. The
//! endpoint is process-wide and its server runs on whichever runtime started it,
//! so two `#[tokio::test]`s in one binary take each other's server down; two
//! binaries have a process each.

mod suite;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use notagent::core::mcp::client::McpConnection;
use notagent::core::mcp::{McpHttpServer, McpServerConfig};
use notagent::core::modes::shells::{ApprovalLevel, ShellId};
use notagent::core::permissions::coordinator::ApprovalPresenter;
use notagent::core::permissions::gate::{PermissionGate, PermissionGateOptions};
use notagent::core::permissions::hook::PermissionSessionState;
use notagent::core::permissions::request::ApprovalAnswer;

use suite::{HarnessOptions, create_harness};

/// A gate whose approval dialog always answers the same way, counting how often
/// it was asked.
fn gate(
    cwd: &str,
    approval: ApprovalLevel,
    answer: ApprovalAnswer,
) -> (Arc<PermissionGate>, Arc<AtomicUsize>) {
    let asked = Arc::new(AtomicUsize::new(0));
    let counting = Arc::clone(&asked);
    let present: ApprovalPresenter = Arc::new(move |_request| {
        let counting = Arc::clone(&counting);
        let answer = answer;
        Box::pin(async move {
            counting.fetch_add(1, Ordering::SeqCst);
            answer
        })
    });
    let cwd = cwd.to_owned();
    let state = Arc::new(move || PermissionSessionState {
        mode_id: Some("manual".to_owned()),
        shell: Some(ShellId::Worker),
        approval,
        cwd: cwd.clone(),
    });
    let gate = PermissionGate::new(PermissionGateOptions {
        state,
        present,
        policies: Vec::new(),
        decide: None,
    });
    (Arc::new(gate), asked)
}

fn client_config(url: &str, authorization: &str) -> McpServerConfig {
    let mut headers = std::collections::BTreeMap::new();
    headers.insert("Authorization".to_owned(), authorization.to_owned());
    McpServerConfig::Http(McpHttpServer {
        url: url.to_owned(),
        headers,
        timeout: Some(10),
        startup_timeout: Some(10),
        ..McpHttpServer::default()
    })
}

/// Both cases in one test, for the reason given in the module header: the
/// endpoint is process-wide and its server runs on the runtime that started it,
/// so a second `#[tokio::test]` in this binary takes it down under the first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_borrowed_call_meets_the_permission_chain() {
    let workspace = tempfile::TempDir::new().expect("a workspace");
    let cwd = workspace.path().to_string_lossy().into_owned();
    // Deny whatever is asked. A write outside the working directory is the
    // ordinary case that reaches the dialog rather than passing on a rule.
    let (permissions, asked) = gate(&cwd, ApprovalLevel::Manual, ApprovalAnswer::Deny);

    let harness = create_harness(HarnessOptions {
        permissions: Some(Arc::clone(&permissions)),
        ..HarnessOptions::default()
    });

    let endpoint = harness.session.lend_tools().await.expect("lends");

    let connection = McpConnection::connect(
        &client_config(&endpoint.url, &endpoint.authorization),
        &std::collections::BTreeMap::new(),
    )
    .await
    .expect("connects");

    // Outside the working directory on purpose: an in-directory write passes on
    // a rule without reaching the dialog, which would prove nothing about
    // whether the chain ran at all.
    let elsewhere = tempfile::TempDir::new().expect("somewhere else");
    let target = elsewhere.path().join("outside.txt");
    let mut arguments = serde_json::Map::new();
    arguments.insert(
        "path".to_owned(),
        serde_json::json!(target.to_string_lossy()),
    );
    arguments.insert("content".to_owned(), serde_json::json!("borrowed"));

    let outcome = connection
        .call_tool("write", arguments)
        .await
        .expect("the endpoint answers");

    // Refused, and the file was never written — the whole point.
    assert_eq!(outcome.is_error, Some(true));
    assert!(
        !target.exists(),
        "a refused borrowed call wrote the file anyway"
    );
    assert_eq!(
        asked.load(Ordering::SeqCst),
        1,
        "the borrowed call did not reach the approval dialog"
    );

    // And a session with no chain at all refuses rather than running ungated:
    // an endpoint that quietly drops the check in some configurations is worse
    // than one that does not serve there.
    let ungated = create_harness(HarnessOptions::default());
    let open = ungated.session.lend_tools().await.expect("lends");

    let connection = McpConnection::connect(
        &client_config(&open.url, &open.authorization),
        &std::collections::BTreeMap::new(),
    )
    .await
    .expect("connects");

    let mut arguments = serde_json::Map::new();
    arguments.insert("path".to_owned(), serde_json::json!("a.txt"));
    arguments.insert("content".to_owned(), serde_json::json!("borrowed"));

    let outcome = connection.call_tool("write", arguments).await;

    let refused = match outcome {
        Ok(result) => {
            assert_eq!(result.is_error, Some(true));
            result
                .content
                .iter()
                .filter_map(|block| block.as_text().map(|text| text.text.clone()))
                .collect::<Vec<_>>()
                .join("")
        }
        Err(error) => error.to_string(),
    };
    assert!(refused.contains("permission chain"), "{refused}");
}
