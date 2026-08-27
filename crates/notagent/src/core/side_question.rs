//! A question the conversation does not have to carry.
//!
//! A question asked in the main conversation is paid for forever: it is re-sent
//! with every later request and it moves the compaction threshold closer. A
//! side question is asked of a child that already knows everything the main
//! agent knows, answered from that knowledge alone, and then dropped. Nothing
//! reaches the main context — no message, no reminder, no usage.
//!
//! # Why the child is built the way it is
//!
//! The child shares the parent's entire prefix, and every obvious shortcut in
//! building it destroys that. The three that matter:
//!
//! - **Its tools are the parent's, unfiltered and in the parent's order.** The
//!   Anthropic path puts its cache breakpoint on the last tool of the list
//!   (`notagent-ai/src/api/anthropic_params.rs`), so a shorter list moves the
//!   breakpoint and invalidates everything behind it. The child is stopped from
//!   *calling* a tool instead, at execution time, by [`refuse_tool_calls`].
//! - **Its system prompt is the parent's, unchanged.** The prompt is the head
//!   of the cached prefix; appending so much as a paragraph rewrites all of it.
//!   The instruction below therefore travels as the last message rather than as
//!   part of the prompt.
//! - **Its session id is the parent's.** That id becomes the OpenAI
//!   `prompt_cache_key`, the Anthropic `x-session-affinity` header and the
//!   session header of a local inference server. [`super::delegation::run`]
//!   deliberately mints a *fresh* one for a subagent, and is right to: a
//!   subagent shares no prefix with its parent. This child shares all of it,
//!   so the same decision made here would tell a prefix cache that a
//!   conversation it already holds is a stranger.

use std::sync::Arc;

use notagent_agent::agent::{Agent, AgentOptions};
use notagent_agent::types::{
    AgentMessage, AgentTool, BeforeToolCallFn, BeforeToolCallResult, BoxFuture,
};
use notagent_ai::types::{TextContent, TextOrImageContent, UserContent};

use crate::core::messages::CustomMessage;

/// What a blocked tool call reports back to the model.
pub const TOOL_CALL_DISABLED_MESSAGE: &str =
    "Tool calls are disabled for side questions. Answer with text only.";

/// The custom-message type the side-channel instruction is recorded under.
pub const SIDE_QUESTION_REMINDER_TYPE: &str = "side_question";

/// What the child is told about the conversation it has been dropped into.
///
/// The paragraph about the tools is not decoration: the child can see every
/// tool the parent has and would otherwise reasonably try one. It is told both
/// that they are refused and why they are still there, so it does not read
/// their presence as permission.
pub const SIDE_QUESTION_INSTRUCTION: &str = "\
This is a side-channel conversation with the user. Answer their question \
directly, from what you already know.

Specifically:
- You are a separate, lightweight instance. The main agent is still working and \
does not know about this exchange; do not refer to having been interrupted.
- Do not call any tools. Every tool call is refused. The tool definitions are \
present in this request only so that the cached prefix stays intact — their \
presence is not permission to use them.
- Answer only with text, from this conversation and what you already know.
- There may be follow-up questions in this side channel.
- If you do not know, say so plainly rather than guessing or offering to find \
out.";

/// The instruction as the message that carries it.
///
/// Undisplayed: the panel shows the exchange, and the instruction is machinery
/// rather than something the user asked to read.
pub fn side_question_instruction_message() -> CustomMessage {
    CustomMessage {
        custom_type: SIDE_QUESTION_REMINDER_TYPE.to_string(),
        content: UserContent::Blocks(vec![TextOrImageContent::Text(TextContent::new(
            SIDE_QUESTION_INSTRUCTION,
        ))]),
        display: false,
        details: None,
        timestamp: chrono::Utc::now().timestamp_millis(),
    }
}

/// A hook that refuses every tool call with [`TOOL_CALL_DISABLED_MESSAGE`].
///
/// Refusing here rather than withholding the tools is what keeps the cached
/// prefix intact; see the module documentation.
pub fn refuse_tool_calls() -> BeforeToolCallFn {
    Arc::new(|_context, _signal| {
        Box::pin(async move {
            Some(BeforeToolCallResult {
                block: Some(true),
                reason: Some(TOOL_CALL_DISABLED_MESSAGE.to_string()),
                // Not terminating: a refused call is something the child reads
                // and answers around, not a reason to end its turn.
                terminate: None,
            })
        }) as BoxFuture<'static, Option<BeforeToolCallResult>>
    })
}

/// Everything the fork needs from the session that owns the parent.
pub struct SideQuestionFork {
    pub parent: Arc<Agent>,
    /// The parent's cache identity, which the child inherits.
    pub session_id: String,
    /// The parent's messages as of the fork. Only complete messages: a partial
    /// tail would differ from what the parent eventually commits, which is
    /// exactly the prefix both of them are relying on.
    pub history: Vec<AgentMessage>,
}

/// Builds the child.
///
/// Every provider-facing field comes from the parent, so the child talks to the
/// same model through the same transport, retries and headers. The one
/// deliberate addition is the tool refusal; the one deliberate omission is the
/// parent's own `before_tool_call`, which is replaced rather than chained
/// because nothing the child does may reach a permission prompt or a hook.
pub fn create_side_question_agent(fork: &SideQuestionFork) -> Arc<Agent> {
    let parent_state = fork.parent.state();
    let parent_options = fork.parent.options();
    // The parent's tools, unfiltered and in order. See the module docs.
    let tools: Vec<Arc<dyn AgentTool>> = parent_state.tools.clone();

    let child = Agent::new(AgentOptions {
        before_tool_call: Some(refuse_tool_calls()),
        system_prompt: Some(parent_state.system_prompt.clone()),
        model: Some(parent_state.model.clone()),
        thinking_level: Some(parent_state.thinking_level),
        tools: Some(tools),
        // The parent's id, not a fresh one — the two share their whole prefix.
        session_id: Some(fork.session_id.clone()),
        convert_to_llm: parent_options.convert_to_llm.clone(),
        stream_fn: parent_options.stream_fn.clone(),
        get_api_key: parent_options.get_api_key.clone(),
        on_payload: parent_options.on_payload.clone(),
        on_response: parent_options.on_response.clone(),
        thinking_budgets: parent_options.thinking_budgets,
        transport: parent_options.transport,
        max_retry_delay_ms: parent_options.max_retry_delay_ms,
        tool_execution: parent_options.tool_execution,
        ..AgentOptions::default()
    });

    // The inherited history, then the instruction — in that order and never the
    // other way round. The conversation breakpoint sits on the last message, so
    // appending leaves everything before it byte-identical.
    let mut messages = fork.history.clone();
    messages.push(AgentMessage::Custom(side_question_instruction_message()));
    child.set_messages(messages);
    child
}

/// The parent's messages as of now, with an unfinished tail left out.
///
/// A message the parent is still streaming is not part of the prefix yet: the
/// parent will commit a longer version of it, and a child that inherited the
/// short one would share a prefix with nobody.
pub fn complete_messages(parent: &Agent) -> Vec<AgentMessage> {
    let state = parent.state();
    let streaming_tail = state
        .streaming_message
        .as_ref()
        .and_then(|message| match message {
            AgentMessage::Assistant(_) => Some(()),
            _ => None,
        })
        .is_some();
    let mut messages = state.messages;
    if streaming_tail
        && matches!(messages.last(), Some(AgentMessage::Assistant(_)))
        && state.is_streaming
    {
        messages.pop();
    }
    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_instruction_says_why_the_tools_are_there() {
        // A child that reads their presence as permission is the failure this
        // paragraph exists to prevent.
        assert!(SIDE_QUESTION_INSTRUCTION.contains("cached prefix"));
        assert!(SIDE_QUESTION_INSTRUCTION.contains("not permission"));
        assert!(SIDE_QUESTION_INSTRUCTION.contains("Do not call any tools"));
    }

    #[test]
    fn the_instruction_travels_as_an_undisplayed_message() {
        let message = side_question_instruction_message();
        assert_eq!(message.custom_type, SIDE_QUESTION_REMINDER_TYPE);
        assert!(!message.display, "the panel shows the exchange, not this");
    }
}
