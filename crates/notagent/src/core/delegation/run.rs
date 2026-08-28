use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use notagent_agent::agent::{Agent, AgentOptions};
use notagent_agent::types::{AgentEvent, AgentMessage, AgentTool};
use notagent_ai::types::Model;
use notagent_ai::uuidv7;
use tokio_util::sync::CancellationToken;

use crate::core::delegation::agent_type::SubagentType;
use crate::core::permissions::requester::{Requester, with_requester};
use crate::core::tools::{ToolName, ToolsOptions, create_tool};

/// Tools a child never gets, whatever its mode allows.
/// Delegation is the first: a subagent that can delegate turns one conversation
/// into a tree of them, each retelling the one above it, and a task that reaches
/// the fourth level bears no relation to what was asked.
/// The three background tools are the second: a subagent's conversation ends
/// when it answers, and work it detached would outlive it with nobody left to
/// collect the result.
/// its agent after every turn, and a child running that loop inside its
/// parent's multiplies turns with nobody watching. All three exclusions are
/// structural because an instruction is something a model can decide to ignore.
const TOOLS_WITHHELD_FROM_CHILDREN: [ToolName; 7] = [
    ToolName::Task,
    ToolName::TaskList,
    ToolName::TaskOutput,
    ToolName::TaskStop,
    // A checklist exists to show the user progress, and nobody watches a
    // subagent. Its list would also outlive nothing — the conversation ends
    // with its answer.
    ToolName::TodoWrite,
    ToolName::CreateGoal,
    ToolName::UpdateGoal,
];

/// Longest a child may run before it is stopped.
/// Nothing else bounds it: a child has no user watching it and no turn ceiling
/// of its own, so a model that loops in a subagent loops until the provider
/// refuses. Two hours matches the reference and is far longer than any task
/// worth delegating, which is the point — this is a backstop, not a schedule.
pub const DELEGATION_TIMEOUT_MS: u64 = 2 * 60 * 60 * 1000;

/// Env override for the deadline. Zero means no deadline at all.
pub const DELEGATION_TIMEOUT_ENV: &str = "NOTAGENT_SUBAGENT_TIMEOUT_MS";

pub fn resolve_delegation_timeout_ms() -> u64 {
    let Ok(raw) = std::env::var(DELEGATION_TIMEOUT_ENV) else {
        return DELEGATION_TIMEOUT_MS;
    };
    if raw.trim().is_empty() {
        return DELEGATION_TIMEOUT_MS;
    }
    // `Number(raw)` accepts leading and trailing whitespace and rejects the
    // rest; only a non-negative integer is taken.
    match raw.trim().parse::<f64>() {
        Ok(parsed) if parsed.fract() == 0.0 && parsed >= 0.0 && parsed.is_finite() => parsed as u64,
        _ => DELEGATION_TIMEOUT_MS,
    }
}

/// Longest answer a child may return.
/// A subagent exists to compress work into a paragraph. One that returns a
/// megabyte has failed at that regardless of what it says, and passing it on
/// would spend the parent's context on the very thing delegation was meant to
/// keep out of it.
pub const MAX_ANSWER_CHARS: usize = 100_000;

/// Shortest answer a child may return without being asked to expand it.
/// The two caps point at opposite failures and both are real. A megabyte is a
/// transcript the parent has to read anyway; three words is a handoff the parent
/// cannot act on. One extra turn is cheaper than the parent redoing the
/// investigation to find out what actually happened.
pub const MIN_ANSWER_CHARS: usize = 200;

const EXPANSION_PROMPT: &str = concat!(
    "That answer is too short to hand back. It is the only thing the agent that delegated this will see — it cannot read your conversation.",
    " Restate it completely: what you found or changed, the path of every file involved, how you verified it, and anything still open.",
    " Do not repeat the task description back; report the outcome.",
);

#[derive(Debug, Clone)]
pub struct DelegationRun {
    /// Identifies this child, so a later call can continue it.
    pub session_id: String,
    /// The child's final answer, or an explanation of why there is none.
    pub text: String,
    /// Whether the run ended without producing an answer.
    pub failed: bool,
    /// What the child said and did, kept so the same child can be continued.
    pub transcript: Vec<AgentMessage>,
}

/// Resolves a tool from the parent's assembled registry.
pub type ResolveToolFn = Arc<dyn Fn(ToolName) -> Option<Arc<dyn AgentTool>> + Send + Sync>;
/// Reports the child's running token total as its conversation grows.
pub type OnTokensFn = Arc<dyn Fn(u64) + Send + Sync>;

/// A skill a child is told to follow, already read from disk.
/// Resolved by the caller rather than here: the session owns the skill
/// catalogue, and a child that resolved names itself could reach a skill the
/// delegating session cannot see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildSkill {
    pub name: String,
    /// Where it came from, so relative references inside the body resolve.
    pub path: String,
    pub body: String,
}

pub struct DelegationOptions {
    pub parent: Arc<Agent>,
    /// What the child may do. Replaces the mode this used to take.
    pub agent: SubagentType,
    /// Skills prepended to the task, in the order the caller named them.
    pub skills: Vec<ChildSkill>,
    /// The star name this child is shown under. Travels with the run so an
    /// approval prompt raised by the child can say who is asking.
    pub alias: String,
    pub cwd: String,
    pub task: String,
    /// Transcript of a previous run, when continuing one.
    pub history: Option<Vec<AgentMessage>>,
    /// Identifier to keep when continuing; a new one is minted otherwise.
    pub session_id: Option<String>,
    pub signal: Option<CancellationToken>,
    /// Tool construction options, so a child reads files the way the parent does.
    pub tool_options: Option<ToolsOptions>,
    /// Without it a child sees only the built-ins. The built-in factory remains
    /// the fallback.
    pub resolve_tool: Option<ResolveToolFn>,
    /// Overrides the ceiling, for tests and for callers with their own bound.
    pub timeout_ms: Option<u64>,
    pub on_tokens: Option<OnTokensFn>,
    /// The model the child binds instead of the parent's. Addition over the
    /// it; `None` inherits the parent's model as before.
    pub model_override: Option<Model>,
}

/// The child's tools: its type's allowlist, minus what no child may have.
pub fn child_tool_names(agent: SubagentType) -> Vec<ToolName> {
    agent
        .tools()
        .into_iter()
        .filter(|name| !TOOLS_WITHHELD_FROM_CHILDREN.contains(name))
        .collect()
}

/// One skill in the same envelope the `skill` tool produces.
/// Deliberately identical: a child that was handed a skill up front and one
/// that loaded the same skill on request should be reading the same thing, or
/// guidance written for one path would quietly misbehave on the other.
fn render_skill_block(skill: &ChildSkill) -> String {
    format!(
        "<skill name=\"{}\" path=\"{}\">\n{}\n</skill>",
        skill.name,
        skill.path,
        skill.body.trim()
    )
}

/// The first message a child sees: what it is, what it should follow, then the
/// task it was given.
/// The type block states the limit rather than leaving the child to discover it
/// from a missing tool. A read-only child that knows it cannot write reports the
/// change that is needed; one that finds out by having `write` refused tends to
/// spend turns looking for another way through.
pub fn render_child_prompt(agent: SubagentType, skills: &[ChildSkill], task: &str) -> String {
    let mut blocks = vec![format!(
        "<agent type=\"{}\">\n{}\n</agent>",
        agent,
        agent.purpose()
    )];
    blocks.extend(
        skills
            .iter()
            .filter(|skill| !skill.body.trim().is_empty())
            .map(render_skill_block),
    );
    blocks.push(task.to_owned());
    blocks.join("\n\n")
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `text.slice(0, limit)` on UTF-16 code units, never splitting a surrogate
/// pair (JS would; a lone surrogate cannot be represented in a Rust `String`,
/// and the cut lands one unit earlier instead).
fn slice_utf16(text: &str, limit: usize) -> String {
    let mut units = 0;
    let mut end = text.len();
    for (index, character) in text.char_indices() {
        let width = character.len_utf16();
        if units + width > limit {
            end = index;
            break;
        }
        units += width;
    }
    text[..end].to_owned()
}

/// Caps an answer, saying so rather than letting it end mid-sentence.
fn truncate_answer(text: &str) -> String {
    if utf16_len(text) <= MAX_ANSWER_CHARS {
        return text.to_owned();
    }
    format!(
        "{}\n\n[the subagent's answer was truncated at {MAX_ANSWER_CHARS} characters]",
        slice_utf16(text, MAX_ANSWER_CHARS)
    )
}

fn last_assistant_text(messages: &[AgentMessage]) -> String {
    for message in messages.iter().rev() {
        let AgentMessage::Assistant(assistant) = message else {
            continue;
        };
        return assistant
            .content
            .iter()
            .filter_map(|part| match part {
                notagent_ai::types::AssistantContent::Text(text) => Some(text.text.clone()),
                _ => None,
            })
            .collect::<Vec<String>>()
            .join("\n")
            .trim()
            .to_owned();
    }
    String::new()
}

/// Wraps the parent's permission chain so the calls a child makes are known to
/// be the child's.
/// The chain itself is unchanged — that is the point. A child is governed by
/// exactly the rules its parent is, including the approval level, which the
/// chain reads from the live session at every call rather than from anything
/// captured here. So a parent that switches from `auto` to `manual` while a
/// background child is working starts being asked about that child's next call,
/// which is what "inherits the main agent's permissions" has to mean if it is
/// to mean anything.
/// What the wrapper adds is attribution. The scope covers the returned future,
/// so it is still in place while the dialog waits for an answer.
fn attribute_to_child(
    inherited: Option<notagent_agent::types::BeforeToolCallFn>,
    alias: &str,
    agent: SubagentType,
) -> Option<notagent_agent::types::BeforeToolCallFn> {
    let inherited = inherited?;
    let requester = Requester {
        alias: alias.to_owned(),
        agent: agent.as_str().to_owned(),
    };
    Some(Arc::new(move |context, signal| {
        let pending = inherited(context, signal);
        let requester = requester.clone();
        Box::pin(with_requester(requester, pending))
    }))
}

/// Builds the child. Every provider-facing field is taken from the parent so a
/// child talks to the same model through the same transport, retries and
/// headers — anything else would make a subagent behave differently from the
/// agent that spawned it for reasons nobody chose. The one chosen exception:
/// `model_override` (the `/subagent-model` setting) replaces the model alone;
/// transport, retries and headers still come from the parent.
fn create_child(options: &DelegationOptions, session_id: &str) -> Arc<Agent> {
    let parent_state = options.parent.state();
    let parent_options = options.parent.options();
    let tools: Vec<Arc<dyn AgentTool>> = child_tool_names(options.agent)
        .into_iter()
        .map(|name| {
            options
                .resolve_tool
                .as_ref()
                .and_then(|resolve| resolve(name))
                .unwrap_or_else(|| create_tool(name, &options.cwd, options.tool_options.as_ref()))
        })
        .collect();

    let child = Agent::new(AgentOptions {
        before_tool_call: attribute_to_child(
            parent_options.before_tool_call.clone(),
            &options.alias,
            options.agent,
        ),
        system_prompt: Some(parent_state.system_prompt.clone()),
        model: Some(
            options
                .model_override
                .clone()
                .unwrap_or_else(|| parent_state.model.clone()),
        ),
        thinking_level: Some(parent_state.thinking_level),
        tools: Some(tools),
        // A distinct id, not the parent's: the two conversations share no
        // prefix, and a cache-aware backend told otherwise would be told
        // something false.
        session_id: Some(session_id.to_owned()),
        convert_to_llm: parent_options.convert_to_llm.clone(),
        stream_fn: parent_options.stream_fn.clone(),
        get_api_key: parent_options.get_api_key.clone(),
        on_payload: parent_options.on_payload.clone(),
        on_response: parent_options.on_response.clone(),
        after_tool_call: parent_options.after_tool_call.clone(),
        thinking_budgets: parent_options.thinking_budgets,
        transport: parent_options.transport,
        max_retry_delay_ms: parent_options.max_retry_delay_ms,
        tool_execution: parent_options.tool_execution,
        ..AgentOptions::default()
    });

    if let Some(history) = &options.history
        && !history.is_empty()
    {
        child.set_messages(history.clone());
    }
    child
}

/// Runs one child to completion. Never fails: a failed subagent is a result the
/// model has to read and react to, not an error that takes down the delegating
/// tool call and the sibling tasks with it.
pub async fn run_delegation(options: DelegationOptions) -> DelegationRun {
    let session_id = options.session_id.clone().unwrap_or_else(uuidv7);
    let child = create_child(&options, &session_id);
    let timeout_ms = options
        .timeout_ms
        .unwrap_or_else(resolve_delegation_timeout_ms);

    // Every finished assistant message carries what the request cost. Summing
    // them is the child's spend; the parent's own usage is untouched by it.
    let tokens = Arc::new(AtomicU64::new(0));
    let unsubscribe = options.on_tokens.as_ref().map(|on_tokens| {
        let on_tokens = Arc::clone(on_tokens);
        let tokens = Arc::clone(&tokens);
        child.subscribe(Arc::new(move |event, _signal| {
            if let AgentEvent::MessageEnd {
                message: AgentMessage::Assistant(assistant),
            } = &event
            {
                // `usage?.totalTokens ?? 0` — an interrupted stream reports none.
                let spent_now = assistant.usage.total_tokens.unwrap_or(0);
                let spent = tokens.fetch_add(spent_now, Ordering::SeqCst) + spent_now;
                on_tokens(spent);
            }
            Box::pin(async {})
        }))
    });

    // Stopping the child on the turn's signal, and the deadline, are two
    // watchers cancelled together when the run is over.
    let watchers = CancellationToken::new();
    if let Some(signal) = &options.signal {
        let signal = signal.clone();
        let child_handle = Arc::clone(&child);
        let watchers = watchers.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = watchers.cancelled() => {}
                _ = signal.cancelled() => child_handle.abort(),
            }
        });
    }
    let timed_out = Arc::new(AtomicBool::new(false));
    // Zero disables the deadline. Only a deliberate override reaches that, and
    // it exists for runs a human is watching rather than for ordinary use.
    if timeout_ms > 0 {
        let child_handle = Arc::clone(&child);
        let timed_out = Arc::clone(&timed_out);
        let watchers = watchers.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = watchers.cancelled() => {}
                _ = tokio::time::sleep(std::time::Duration::from_millis(timeout_ms)) => {
                    timed_out.store(true, Ordering::SeqCst);
                    child_handle.abort();
                }
            }
        });
    }

    let result = run_child(&options, &child, &session_id, &timed_out, timeout_ms).await;

    watchers.cancel();
    if let Some(unsubscribe) = unsubscribe {
        unsubscribe();
    }
    result
}

async fn run_child(
    options: &DelegationOptions,
    child: &Arc<Agent>,
    session_id: &str,
    timed_out: &AtomicBool,
    timeout_ms: u64,
) -> DelegationRun {
    let aborted = || {
        options
            .signal
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
    };
    if aborted() {
        return DelegationRun {
            session_id: session_id.to_owned(),
            text: "The turn was interrupted before this task started.".to_owned(),
            failed: true,
            transcript: child.state().messages,
        };
    }
    // A continued child already carries its mode; repeating the block every
    // time would grow the transcript with guidance it is already following.
    let prompt = match &options.history {
        Some(history) if !history.is_empty() => options.task.clone(),
        _ => render_child_prompt(options.agent, &options.skills, &options.task),
    };
    if let Err(error) = child
        .prompt_text(prompt, None, chrono::Utc::now().timestamp_millis())
        .await
    {
        return DelegationRun {
            session_id: session_id.to_owned(),
            text: format!("The subagent failed: {error}"),
            failed: true,
            transcript: child.state().messages,
        };
    }

    // One chance to expand a handoff too thin to act on. Only one: a child that
    // is still terse after being told why is not going to improve, and asking
    // again would spend the parent's money on the same paragraph.
    let mut answer = last_assistant_text(&child.state().messages);
    if !timed_out.load(Ordering::SeqCst)
        && !aborted()
        && !answer.is_empty()
        && utf16_len(&answer) < MIN_ANSWER_CHARS
    {
        if let Err(error) = child
            .prompt_text(
                EXPANSION_PROMPT,
                None,
                chrono::Utc::now().timestamp_millis(),
            )
            .await
        {
            return DelegationRun {
                session_id: session_id.to_owned(),
                text: format!("The subagent failed: {error}"),
                failed: true,
                transcript: child.state().messages,
            };
        }
        let expanded = last_assistant_text(&child.state().messages);
        if !expanded.is_empty() {
            answer = expanded;
        }
    }

    let transcript = child.state().messages;
    let text = truncate_answer(&answer);
    if timed_out.load(Ordering::SeqCst) {
        // The partial text is deliberately not returned. A subagent stopped
        // mid-thought reports whatever it happened to be saying, which reads
        // like a conclusion and is not one; continuing the same child keeps
        // everything it learned and costs less than acting on a fragment.
        return DelegationRun {
            session_id: session_id.to_owned(),
            text: [
                format!(
                    "The subagent reached its {} deadline and was stopped before answering.",
                    describe_duration(timeout_ms)
                ),
                format!(
                    "Continue it with session_id \"{session_id}\" and agent \"{}\" — it keeps everything it had learned.",
                    options.agent
                ),
                "Do not start the same work over from scratch.".to_owned(),
            ]
            .join(" "),
            failed: true,
            transcript,
        };
    }
    if text.is_empty() {
        return DelegationRun {
            session_id: session_id.to_owned(),
            text: if aborted() {
                "The turn was interrupted before this task finished.".to_owned()
            } else {
                "The subagent produced no answer.".to_owned()
            },
            failed: true,
            transcript,
        };
    }
    DelegationRun {
        session_id: session_id.to_owned(),
        text,
        failed: false,
        transcript,
    }
}

/// A deadline in words, so the message says two hours rather than 7200000.
fn describe_duration(ms: u64) -> String {
    if ms.is_multiple_of(3_600_000) {
        let hours = ms / 3_600_000;
        return format!("{hours} hour{}", if hours == 1 { "" } else { "s" });
    }
    if ms.is_multiple_of(60_000) {
        let minutes = ms / 60_000;
        return format!("{minutes} minute{}", if minutes == 1 { "" } else { "s" });
    }
    let seconds = (ms as f64 / 1000.0 + 0.5).floor() as u64;
    format!("{seconds} second{}", if seconds == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::permissions::requester::current_requester;

    fn tool_context() -> notagent_agent::types::BeforeToolCallContext {
        notagent_agent::types::BeforeToolCallContext {
            assistant_message: notagent_ai::types::AssistantMessage {
                content: Vec::new(),
                api: "anthropic-messages".to_owned(),
                provider: "anthropic".to_owned(),
                model: "mock".to_owned(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: notagent_ai::types::Usage::default(),
                stop_reason: notagent_ai::types::StopReason::Stop,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                end_turn: None,
                timestamp: 0,
            },
            tool_call: notagent_ai::types::ToolCall {
                id: "call-1".to_owned(),
                name: "read".to_owned(),
                ..Default::default()
            },
            args: serde_json::Value::Null,
            context: notagent_agent::types::AgentContext::default(),
        }
    }

    /// The attribution has to be readable from inside the awaited future, not
    /// just at the moment the hook is entered: the real chain reads it after
    /// the policy evaluation, and keeps holding it while a dialog waits for an
    /// answer.
    #[tokio::test]
    async fn names_the_child_behind_a_call_for_the_whole_evaluation() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let inherited: notagent_agent::types::BeforeToolCallFn =
            Arc::new(move |_context, _signal| {
                let sink = Arc::clone(&sink);
                Box::pin(async move {
                    tokio::task::yield_now().await;
                    sink.lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(current_requester());
                    None
                })
            });

        let wrapped = attribute_to_child(Some(inherited), "Wolf 359", SubagentType::ReadOnly)
            .expect("a chain to wrap");
        wrapped(tool_context(), None).await;

        let seen = seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(
            seen,
            vec![Some(Requester {
                alias: "Wolf 359".to_owned(),
                agent: "read-only".to_owned(),
            })]
        );
    }

    /// A session with no chain gets no wrapper. Manufacturing one would put a
    /// hook in front of a session that deliberately has none.
    #[test]
    fn adds_no_chain_where_the_parent_had_none() {
        assert!(attribute_to_child(None, "Vega", SubagentType::Worker).is_none());
    }
}
