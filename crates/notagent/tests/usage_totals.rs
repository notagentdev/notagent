//! Port of the `getUsageCostBreakdown` case of
//! `packages/coding-agent/test/agent-session-stats.test.ts` ("groups tool and
//! summary usage separately from model-attributed usage"). The remaining cases
//! of that file exercise `AgentSession.getSessionStats` and belong to workstream
//! C's agent-session suites.

use notagent::core::session_manager::SessionManager;
use notagent::core::usage_totals::{UsageCostBreakdownEntry, get_usage_cost_breakdown};
use notagent_agent::types::AgentMessage;
use notagent_ai::types::{
    AssistantContent, AssistantMessage, StopReason, TextContent, TextOrImageContent,
    ToolResultMessage, Usage, UsageCost, UserContent, UserMessage,
};

const PROVIDER: &str = "anthropic";
const MODEL: &str = "claude-sonnet-4-5";

fn create_usage(total_tokens: u64, total_cost: f64) -> Usage {
    Usage {
        input: total_tokens,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        cache_write1h: None,
        reasoning: None,
        total_tokens: Some(total_tokens),
        cost: UsageCost {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
            total: total_cost,
        },
    }
}

fn user_message(text: &str, timestamp: i64) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: UserContent::Text(text.to_owned()),
        timestamp,
    })
}

fn assistant_message(text: &str, usage: Usage, timestamp: i64) -> AgentMessage {
    AgentMessage::Assistant(AssistantMessage {
        content: vec![AssistantContent::Text(TextContent {
            text: text.to_owned(),
            ..TextContent::default()
        })],
        api: "anthropic-messages".to_owned(),
        provider: PROVIDER.to_owned(),
        model: MODEL.to_owned(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage,
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp,
    })
}

fn tool_result_message(usage: Usage) -> AgentMessage {
    AgentMessage::ToolResult(ToolResultMessage {
        tool_call_id: "tool-call-1".to_owned(),
        tool_name: "test_tool".to_owned(),
        content: vec![TextOrImageContent::Text(TextContent {
            text: "tool result".to_owned(),
            ..TextContent::default()
        })],
        details: None,
        usage: Some(usage),
        added_tool_names: None,
        is_error: false,
        timestamp: 1,
    })
}

#[test]
fn groups_tool_and_summary_usage_separately_from_model_attributed_usage() {
    let mut session_manager = SessionManager::in_memory(None, None).expect("session");
    let root_id = session_manager
        .append_message(&user_message("hello", 1))
        .expect("user");
    session_manager
        .append_message(&assistant_message("response", create_usage(100, 0.5), 2))
        .expect("assistant");
    session_manager
        .append_message(&tool_result_message(create_usage(100, 1.0)))
        .expect("tool result");
    session_manager
        .append_compaction(
            "summary",
            &root_id,
            None,
            100,
            None,
            None,
            Some(false),
            Some(create_usage(100, 2.0)),
        )
        .expect("compaction");
    session_manager
        .branch_with_summary(
            None,
            "branch summary",
            None,
            Some(false),
            Some(create_usage(100, 3.0)),
        )
        .expect("branch summary");

    assert_eq!(
        get_usage_cost_breakdown(&session_manager.get_entries()),
        vec![
            UsageCostBreakdownEntry {
                key: "Tools/summaries".to_owned(),
                cost: 6.0,
                tokens: 300,
            },
            UsageCostBreakdownEntry {
                key: format!("{PROVIDER}/{MODEL}"),
                cost: 0.5,
                tokens: 100,
            },
        ]
    );
}
