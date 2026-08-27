# Side-Question Panel (`/btw`)

## Objective

Give the user a way to ask a question that the running conversation should not
have to carry: a forked child agent that already knows everything the main agent
knows, answers from that knowledge in text alone, and disappears without leaving
a trace in the main context.

The feature exists because of what it does *not* do. A question asked in the
main conversation costs context forever after — it is re-sent with every later
request, and it pushes the compaction threshold closer. A side question costs
one exchange that nobody else has to read again.

Two properties are load-bearing and everything below is subordinate to them:

1. **The main agent's context is not touched.** No message appended, no reminder
   injected, no tool list changed. Its next request must be byte-identical to
   what it would have been had the side question never happened.
2. **Neither agent's prompt cache is damaged.** The child shares the parent's
   whole prefix and must be constructed so the provider sees that prefix
   unchanged — same system prompt, same tools in the same order, same cache
   identity, and its own additions strictly at the tail.

Expected outcome: `/btw <question>` opens a bordered panel above the editor,
streams an answer into it, accepts follow-up questions in the same panel, and
closes on Escape without the main conversation having changed.

## Background: what makes this cache-safe or not

The constraints are not abstract; each one is a specific line in this
repository.

**The tool list is part of the cached prefix.** The Anthropic path puts the
cache breakpoint on the *last* tool in the list
(`crates/notagent-ai/src/api/anthropic_params.rs:497`). Removing tools from the
child — the obvious way to stop it calling them — moves that breakpoint and
invalidates the entire cached prefix behind it. The child must therefore be
given the parent's tool list unchanged, in the same order, and be prevented from
*calling* them by another means.

**The system prompt is the head of the prefix.** Any edit to it, including
appending a paragraph, invalidates everything. The side-channel instruction
therefore cannot live in the system prompt.

**The conversation breakpoint sits on the last user message**
(`crates/notagent-ai/src/api/anthropic_params.rs:417`). An instruction appended
after the inherited history leaves every earlier message byte-identical, so the
cache is read rather than rewritten. An instruction *inserted* anywhere earlier
would rewrite everything after it.

**Cache identity travels in `session_id`.** It becomes the OpenAI
`prompt_cache_key` (`crates/notagent-ai/src/api/openai_completions_params.rs:860`),
the Anthropic `x-session-affinity` header
(`crates/notagent-ai/src/api/anthropic_params.rs:842`), and the session header of
the local inference provider
(`crates/notagent-ai/src/providers/mtplx.rs`, `SessionAffinityFormat::Mtplx`).
The existing delegation path deliberately mints a *fresh* id for a child
(`crates/notagent/src/core/delegation/run.rs:339`) with the stated reason that
parent and child share no prefix — a correct decision there and exactly the
wrong one here. A side-question child shares the entire prefix and must inherit
the parent's id, or a server-side prefix cache will treat it as a stranger.

**Tool calls are refused at execution time.** The loop already offers the hook:
`before_tool_call` may return a block with a reason, which the loop turns into a
tool error the model reads (`crates/notagent-agent/src/agent_loop.rs:910`).
That is how the child keeps the tools in its request while never running one.

## Implementation Plan

- [ ] 1. Add a `core/side_question.rs` module holding the feature's constants
      and its child-construction rule. It owns the side-channel instruction
      text, the refusal message a blocked tool call returns, and the custom
      message type the instruction is recorded under. Keeping the text in one
      module — as `core/session_init.rs` does for its own brief — means the
      wording can be reviewed as prose rather than hunted through call sites.
      The instruction must tell the model four things: that this is a
      side-channel conversation with the user; that it answers from what it
      already knows; that it must not call tools, and that the tool definitions
      it can see are present only so the cached prefix stays intact; and that it
      should say so plainly when it does not know.

- [ ] 2. Build the child agent from a snapshot of the parent, changing nothing
      that the provider hashes. Model the construction on
      `crates/notagent/src/core/delegation/run.rs:307` but invert its three
      central decisions: the child takes the parent's system prompt *as is*, the
      parent's full tool list in the parent's order rather than a
      subagent-type allowlist, and the parent's `session_id` rather than a fresh
      one. Record in the module documentation why each of the three differs from
      delegation, so a later reader who copies from delegation again does not
      reintroduce the fresh id.

- [ ] 3. Decide and document what "snapshot of the parent" means while the main
      agent is streaming. The main agent's message list is in flux mid-turn and
      its last assistant message may be partial. Take only the messages that are
      complete at the moment of the fork, so the child never inherits half a
      message — a partial tail would both confuse the child and, being different
      from what the parent will eventually commit, defeat the shared prefix. If
      it turns out that a clean snapshot cannot be taken while streaming, the
      fallback is to refuse `/btw` during a turn with a message saying why;
      prefer the snapshot if it is available.

- [ ] 4. Append the side-channel instruction as the child's last message rather
      than merging it into the system prompt, using the same undisplayed
      custom-message shape the session already uses for reminders
      (`crates/notagent/src/core/agent_session.rs`, `send_custom_message` with
      `display: false`). Assert in a test that the instruction is the final
      entry and that everything before it equals the parent's snapshot exactly.

- [ ] 5. Refuse every tool call the child makes through the loop's
      `before_tool_call` hook, returning the refusal message from task 1 as the
      block reason. Do not filter the tool list to achieve this. The refusal
      text is what the model reads back, so it should state that tool calls are
      disabled for side questions and that the answer must be text.

- [ ] 6. Expose the feature on `AgentSession` as three operations: start a side
      question, ask a follow-up in the existing child, and cancel or dispose the
      child. Follow-ups reuse the same child so the panel reads as one
      conversation and the child's own prefix stays warm across turns. Model the
      lifecycle on the `/init` run in `crates/notagent/src/core/agent_session.rs`
      (`run_init`) for how a child is driven and cancelled, but do not register
      the child with the task manager: a side question is not background work
      and has no business in the task panel.

- [ ] 7. Guarantee that nothing flows back. The child's messages, events and
      usage must not reach the main agent's message list, its session file, or
      its context-usage figures. Add a test that runs a side question to
      completion and asserts the parent's message list and its serialised
      session entries are identical before and after — this is the test that
      would catch a future refactor quietly wiring the child into the parent.

- [ ] 8. Register `/btw` in `crates/notagent/src/core/slash_commands.rs:62`
      with an argument hint for the question, and dispatch it in
      `crates/notagent/src/modes/interactive/interactive_mode.rs` beside the
      other command arms. Invoking it while a panel is already open replaces
      that panel's child rather than stacking a second one.

- [ ] 9. Add the panel component under
      `crates/notagent/src/modes/interactive/components/`. It renders as a
      bordered box that connects to the editor below it: a top border carrying
      an accent-coloured title and a muted key hint, body lines framed left and
      right, and no bottom border, so the editor's own frame closes the box. The
      hint names Escape to close, and adds the scroll keys only when the body is
      actually taller than the panel. Each exchange renders as the question on
      its own line in the accent colour, then the answer as markdown; while the
      answer is still empty the last few lines of the model's reasoning stand in
      dimmed, and if there is neither, a dimmed line says the answer is being
      waited for. An error renders in the error colour. Exchanges are separated
      by a blank line, and an empty panel shows a dimmed invitation.

- [ ] 10. Bound the panel's height and make it scrollable. It may take at most
      a third of the terminal's rows, with a small floor so it stays usable on a
      short terminal, and it scrolls with the arrow keys when the content
      exceeds that. The panel occupies the existing slot directly above the
      editor (`crates/notagent/src/modes/interactive/interactive_mode.rs:1604`,
      `widget_container_above`), so no layout surgery is needed.

- [ ] 11. Route input while the panel is open. Text submitted in the editor
      becomes a follow-up question to the child instead of a prompt to the main
      agent; the arrow keys scroll the panel; Escape closes it. Submitting while
      the child is still answering shows a transient notice in the panel rather
      than queueing, so the user is not silently waiting on two things at once.

- [ ] 12. Place the panel correctly in the Escape order. It must be handled
      before the compaction, retry and branch-summary targets and before the
      streaming abort in
      `crates/notagent/src/modes/interactive/interactive_mode.rs`
      (`handle_escape`): a panel stacked above the transcript is the thing the
      user means to dismiss, and closing it must not also interrupt the main
      agent's turn. Closing the panel cancels its child if that child is still
      answering.

- [ ] 13. Add cache-invariant tests, which are the point of the exercise. Assert
      that the child's tool list equals the parent's in both content and order,
      that its system prompt is unchanged, that its `session_id` equals the
      parent's, and that its message list is the parent's snapshot plus exactly
      one appended instruction. Each of these is a single comparison and each
      one guards a different way the cache can be lost.

- [ ] 14. Add behaviour tests for the panel: a question produces an answer in
      the panel, a follow-up reaches the same child, a tool call the child
      attempts comes back refused with the documented reason, Escape closes and
      cancels, and the main agent's context is untouched throughout.

- [ ] 15. Document the feature where a user will find it: a short section in
      `README.md` naming what it is for — a question that does not enter the
      conversation — and the fact that closing the panel discards it. Mention
      the setting-free defaults and the keys the panel responds to.

## Verification Criteria

- The child's request carries the parent's system prompt unchanged, the
  parent's tools in the parent's order, and the parent's `session_id`; each is
  asserted separately in a test.
- The child's message list equals the parent's snapshot with exactly one
  appended entry, and that entry is last.
- The main agent's message list and its serialised session entries are
  byte-identical before and after a completed side question.
- A tool call attempted by the child returns the documented refusal as a tool
  error, and no tool is executed.
- The tool list handed to the child is never filtered, which a test asserts by
  comparing lengths and order against the parent's.
- `/btw` with a question opens the panel; a second `/btw` replaces its child
  rather than opening a second panel.
- Escape with the panel open closes the panel and leaves a running main turn
  running.
- The panel never takes more than a third of the terminal's rows and scrolls
  when its content exceeds that.
- `scripts/check.sh` is green, and the pre-existing failure in the resource
  loader test is unchanged.

## Potential Risks and Mitigations

1. **The child is built from the delegation path and inherits its fresh
   session id.** This is the single most likely way to get a silently degraded
   result: everything works, the answer arrives, and only the cache metrics
   would show that the whole prefix was re-read.
   Mitigation: assert the inherited id in a test, and state the reason for the
   difference in the module documentation of both the new module and beside the
   delegation decision it contradicts.

2. **A later change disables tools by filtering the list.** Filtering is the
   obvious implementation and would pass every behavioural test while moving the
   Anthropic breakpoint off the parent's last tool.
   Mitigation: the list-equality test from task 13, plus a comment at the veto
   explaining that the tools are present on purpose.

3. **The instruction drifts into the system prompt.** A future reader may find
   appending a message to the history untidy and move the instruction where
   instructions usually live.
   Mitigation: the test that the instruction is the last message, and a note in
   the module saying that the system prompt is the head of the cached prefix.

4. **A partial assistant message is inherited mid-stream.** The child would get
   half a sentence, and its prefix would differ from what the parent later
   commits.
   Mitigation: task 3 decides this explicitly; whichever way it resolves, it is
   covered by a test that starts a side question during a streaming turn.

5. **The child's own cache write costs more than the feature saves.** The child
   reads the shared prefix but writes a cache entry for its own tail, and a
   one-question side channel may not pay that back.
   Mitigation: accept it and say so — the feature's purpose is to keep the main
   context small, not to be free. Do not add a heuristic that skips the cache
   for short questions; that trades a measurable cost for an unmeasurable one.

6. **The panel and the main turn compete for the editor.** A user typing while
   both are active could send a prompt to the wrong recipient.
   Mitigation: task 11 makes the open panel the sole recipient of submitted
   text, and the panel's presence is visible in the frame above the editor.

## Alternative Approaches

1. **Run the side question through the existing delegation path.** It already
   forks, streams, and cancels. Rejected because its three central decisions are
   all wrong here — it replaces the system prompt with a child brief, restricts
   the tool list by subagent type, and mints a fresh session id — and bending it
   would leave the delegation path harder to read for both callers.

2. **Ask in the main conversation and drop the exchange afterwards.** No child,
   no panel, far less code. Rejected because removing messages from the middle
   of a conversation is exactly what invalidates a cached prefix; the saving in
   context would be paid for in a full re-read on the next turn.

3. **A separate session with no inherited history.** Cheapest to build and
   perfectly isolated. Rejected because the value of the feature is that the
   child already knows the conversation; without it, the user is talking to a
   stranger and would have to explain the context — which is the cost the
   feature exists to avoid.

4. **Keep the side exchange and offer to merge it into the main context on
   close.** Tempting, and it answers the one real drawback: what the user
   learned is lost to the main agent. Rejected for a first version because it
   reintroduces the context cost conditionally and needs its own decision in the
   UI; it can be added later without changing anything below the panel.
