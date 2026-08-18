# Lent tools run like our own

## Objective

A tool borrowed through `/mcp lend` must run exactly as the same tool runs in a
turn of ours: through the permission chain, with the PreToolUse hooks, with the
session context the built-ins read, cancellable by whoever asked for it, and
reported afterwards like any other call.

Today it does none of that. `crates/notagent/src/core/mcp/lend.rs:244` calls the
tool definition directly with no context, no signal and no update callback — and,
because the permission gate is installed on the agent loop rather than on the
tool, the borrowed call never meets it at all. The endpoint therefore runs
`bash`, `write` and `edit` with no approval, in a session whose mode would have
required one. That is the load-bearing finding of this plan and the reason it
exists; the missing context is the smaller half.

The reference is `../notagent-main-rust/crates/notagent_app/src/mcp_lend.rs`,
which routes every borrowed call through the same service entry point its own
turns use, so its permission gate, file tracking and hooks apply unchanged.
`../kimi-code-main` has no lending of any kind; it is a client only. So there is
exactly one reference for this work, and where this port must diverge from it the
plan says why.

Expected outcome: a borrowing agent can do nothing through the endpoint that the
session's own model could not do in a turn, and a user watching the session sees
a borrowed call the way they see any other.

## Assumptions

- The endpoint stays a user-driven, session-long thing (`/mcp lend`, `/mcp lend
  off`), not something bound to a single turn. This differs from the reference,
  where the orchestrator opens it around an external agent it is running
  (`../notagent-main-rust/crates/notagent_app/src/orch.rs:354`), and it is what
  makes the "no turn is running" case below a real one rather than impossible.
- A borrowed call is attributed to the session that lent the tools. There is no
  second identity to attribute it to, and inventing one would put a decision in
  front of the user that nothing else in this port asks them.
- The reference's `rebind`/`release` pair
  (`../notagent-main-rust/crates/notagent_app/src/mcp_lend.rs:264`,
  `:282`) has no counterpart here and must not be copied. It exists because its
  `ToolCallContext` carries the running turn's sender, so a stale one both points
  at nothing and keeps the response stream open forever
  (`../notagent-main-rust/crates/notagent_app/src/orch.rs:396`, `:501`). This
  port's `ToolContext` (`crates/notagent/src/core/tools/tool_definition.rs:34`)
  holds four session-level fields and no sender, and it is produced by a factory
  that resolves through a weak session handle on every call
  (`crates/notagent/src/core/agent_session.rs:1438`). Resolving it per request is
  therefore always current, and there is nothing to rebind or release.

## Implementation Plan

- [ ] 1. **Establish what a borrowed call must pass through, and write it down.**
  Before changing anything, record in the module header of
  `crates/notagent/src/core/mcp/lend.rs` the four things a normal call meets and
  a borrowed one currently does not: the permission chain including its
  PreToolUse hooks (`crates/notagent/src/core/permissions/gate.rs:91`, filled by
  `crates/notagent/src/core/permissions/hook.rs:58`), the session context
  (`crates/notagent/src/core/agent_session.rs:1438`), the caller's cancellation,
  and the PostToolUse reporting driven off agent events
  (`crates/notagent/src/core/hooks/dispatch.rs:190`). Rationale: the gap exists
  because the endpoint reaches past the agent loop to the tool definition, and
  the next person to add a path like this needs the list in front of them rather
  than in a commit message.

- [ ] 2. **Give the lending source a way to run a tool, not just to list one.**
  Widen the `LentToolSource` trait in `crates/notagent/src/core/mcp/lend.rs:45`
  so it answers two questions instead of one: which tools may be borrowed, and
  what happens when one is called. The endpoint keeps deciding nothing on its
  own — it forwards a name, arguments and a cancellation signal, and receives a
  result. Rationale: the permission gate, the hooks and the context all live on
  the session side, and reaching for them from inside the MCP module would tie a
  transport to the session's internals. Keeping the trait as the seam also keeps
  the endpoint testable without a session, which is what
  `crates/notagent/tests/mcp_lend.rs` relies on.

- [ ] 3. **Route the borrowed call through the permission gate.** Implement the
  new trait method on the session side (`SessionLentTools` in
  `crates/notagent/src/core/agent_session.rs`) so it awaits
  `PermissionGate::before_tool_call` (`crates/notagent/src/core/permissions/gate.rs:91`)
  before running anything, and refuses the call with the gate's own reason when
  it blocks. The gate is reachable as
  `crates/notagent/src/core/agent_session_services.rs:170`. A session built
  without one — the SDK and test paths — must refuse borrowed calls outright
  rather than running them ungated: an endpoint that quietly drops the check in
  some configurations is worse than one that does not serve there.
  Rationale: this is the security half of the objective. The chain also carries
  the user's PreToolUse hooks, so one change closes both.

- [ ] 4. **Hand the borrowed call the session context.** Resolve the same
  `ToolContext` a turn would, through the existing factory at
  `crates/notagent/src/core/agent_session.rs:1438` — reusing
  `wrap_tool_definition` (`crates/notagent/src/core/tools/tool_definition.rs:339`)
  rather than building a second path, so there is one place that decides what a
  tool learns about its session. This is what makes `bash` export
  `NOTAGENT_SESSION_ID` and friends (`crates/notagent/src/core/tools/bash.rs:445`)
  and `read` warn correctly about images for the session's model
  (`crates/notagent/src/core/tools/read.rs:557`). Explicitly do not add
  rebind/release: see the third assumption for why the reference needs them and
  this port does not.

- [ ] 5. **Make the caller's cancellation reach the tool.** rmcp cancels
  `RequestContext::ct` when the borrowing client sends a `CancelledNotification`
  (`rmcp-0.10.0/src/service.rs:493`); pass it as the `signal` the tool receives,
  so a borrowing agent that gives up stops the work it started. Decide and record
  whether ending the lend (`/mcp lend off`) or the session should also cancel
  calls already in flight — the token is revoked for new requests today, which
  says nothing about a `bash` that is already running. Rationale: without this a
  borrowed long-running call cannot be stopped from either end, and it is exactly
  the borrowed calls that nobody is watching.

- [ ] 6. **Report a borrowed call the way the session reports its own.** A call
  that arrives over the endpoint currently leaves no trace in the transcript, so
  a user watching the session sees a permission dialog for a tool nothing
  appeared to ask for. Surface it as its own row or notice, naming it as borrowed
  and naming nothing else as borrowed, and fire the PostToolUse hooks that a
  turn's calls get (`crates/notagent/src/core/hooks/dispatch.rs:190`). Decide
  where that sits relative to the agent-event stream in
  `crates/notagent/src/core/agent_session.rs:671` — reusing the event path gets
  hooks and display together, inventing a second path gets neither for free.
  Rationale: an approval prompt with no visible cause is the single worst
  outcome of task 3, because a user faced with one will approve it to make it go
  away.

- [ ] 7. **Settle the two timing cases, and pin them.** First: a borrowed call
  arriving while no turn is running. The approval dialog reaches the UI over the
  channel at `crates/notagent/src/modes/interactive/interactive_mode.rs:281`,
  which does not depend on a turn, but the gate's own documentation ties a
  pending prompt's fate to a turn's cancellation token
  (`crates/notagent/src/core/permissions/gate.rs:86`) — establish what settles a
  prompt that has no turn behind it and make it so, rather than leaving a dialog
  that nothing can dismiss. Second: a borrowed call arriving *during* one of our
  turns, while a dialog is already up. Establish what the approval coordinator
  does with two overlapping requests and record it. Rationale: both are ordinary
  once the endpoint is a session-long thing a user opens, and both end in a stuck
  session if guessed at.

- [ ] 8. **Prove it over the wire.** Extend `crates/notagent/tests/mcp_lend.rs`,
  which already drives the endpoint with this port's own MCP client: a borrowed
  call that the permission chain blocks comes back refused and the tool never
  ran; a borrowed call that it allows runs and reports its result; the tool sees
  the session's context rather than an empty one; a cancelled request stops the
  tool; a session with no permission gate refuses to serve. Keep the existing
  single-test shape and its reason — the endpoint is process-wide and its server
  runs on whichever runtime started it, so a second `#[tokio::test]` takes the
  server down under the first. Rationale: every claim in this plan is about what
  crosses the endpoint, and only a test that crosses it can say whether it holds.

- [ ] 9. **Close it out.** Bump the workspace version, note the divergence from
  the reference's rebind/release in the module header where a reader will meet
  it, and run `./scripts/check.sh` once at the end until format, clippy with
  warnings denied and the whole suite are green.

## Verification Criteria

- A borrowed call to a tool the session's permission mode would prompt for
  produces the same prompt, and the tool does not run until it is answered.
- A borrowed call the chain blocks returns the chain's own reason to the
  borrowing agent, and the tool's side effect has not happened.
- A borrowed `bash` call sees `NOTAGENT_SESSION_ID`, `NOTAGENT_SESSION_FILE`,
  `NOTAGENT_MODEL` and `NOTAGENT_REASONING_LEVEL` with the same values a turn's
  `bash` call would see.
- A session constructed without a permission gate refuses borrowed calls with a
  stated reason rather than running them.
- A borrowing client that cancels its request stops the tool, rather than leaving
  it running to completion.
- A borrowed call appears in the session's transcript, distinguishable from a
  call the model made.
- The PreToolUse and PostToolUse hooks fire for a borrowed call exactly as they
  do for a turn's call.
- A borrowed call arriving with no turn running reaches a decision — approval,
  refusal or a stated timeout — and never leaves a dialog nothing can dismiss.
- `./scripts/check.sh` passes.

## Potential Risks and Mitigations

1. **The approval prompt has no turn to hang from.**
   The gate settles a pending prompt through the turn's cancellation token, and a
   borrowed call has no turn. A prompt that nothing can settle leaves the session
   unable to go idle.
   Mitigation: task 7 makes this the first thing established, not the last.
   Give a borrowed call its own token — the request's, from task 5 — so the
   prompt is settled by the borrowing agent going away, and treat the endpoint
   closing as settling everything outstanding, the way session shutdown already
   does (`crates/notagent/src/core/permissions/gate.rs:114`).

2. **Two callers asking at once.**
   Our own turn and a borrowed call can both reach the gate, and the coordinator
   was written for one asker.
   Mitigation: establish the coordinator's actual behaviour before relying on it
   (task 7), and pin whichever answer it gives with a test rather than assuming
   serialisation.

3. **A borrowed call approved for the wrong reason.**
   A remembered "always allow" answer given for the model's own call would also
   cover a borrowed one, and the user answered it about a different asker.
   Mitigation: decide deliberately in task 6 whether the origin is part of what
   is remembered, and say which way it went in the module header. Whatever is
   chosen, the prompt must name the borrowed call as borrowed so the answer is
   given knowingly.

4. **Refusing to serve breaks the SDK path.**
   Task 3 refuses borrowed calls where no permission gate exists, and the SDK
   builds sessions that way (`crates/notagent/src/core/sdk.rs:62`).
   Mitigation: check who actually lends in those configurations before
   implementing; if the SDK is expected to lend, it must supply a gate rather
   than the endpoint dropping the check. Running ungated is not an option this
   plan leaves open.

5. **The transcript fills with borrowed rows.**
   An agent borrowing tools in a loop could push the session's own output off
   screen.
   Mitigation: treat display volume as a question for task 6 and pick a form
   that collapses repetition, rather than discovering it under a real borrower.

## Alternative Approaches

1. Keep the endpoint ungated and restrict what may be lent to read-only tools.
   Cheaper and closes the hole, but it makes lending useless for the case it
   exists for — an external agent doing work — and it introduces the second tool
   list this port deliberately avoided
   (`crates/notagent/src/core/mcp/lend.rs:19`).
2. Route borrowed calls through the agent loop itself rather than through the
   gate directly, so they pick up hooks, display and permissions with no new
   wiring. Closest to the reference, which calls the same `services.call` its
   turns use. Rejected as the primary path because this port's loop drives a
   model conversation and a borrowed call has none; it would mean synthesising a
   turn around every borrowed call. Worth revisiting if task 6 finds that the
   event path cannot be reused otherwise.
3. Bind the endpoint to a turn, as the reference does, and serve only while one
   is running. Removes risks 1 and 2 entirely. Rejected because `/mcp lend` is a
   command the user issues to hand an endpoint to another program, and an
   endpoint that answers only during our own turns is not one that other program
   can use.
