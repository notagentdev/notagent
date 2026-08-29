# Strengthening Hook Contracts and Lifecycle Coverage

## Objective

Turn the existing hook boundary into an explicit, internally consistent contract: configuration uses unambiguous units, blocking decisions have one structured representation, submitted prompts can be refused before they reach the model, tool executions can be correlated from request through result, and every declared lifecycle event that has a real source in the application is emitted exactly once.

The work keeps the properties that already make the implementation predictable: hooks run sequentially, tool matchers are exact, output is bounded, project and global declarations compose, and a timed-out blocking hook refuses the action. It does not add plugin-owned hooks, regex matching, parallel execution, unbounded output, periodic heartbeat commands, or a hook-driven continuation after the agent has stopped.

No code comment, documentation text, commit subject, or commit body should attribute the design to another implementation. Comments must explain the local constraint or invariant that requires the code's shape.

## Settled Decisions

- `Stop` remains observational. A hook cannot extend or restart an agent run.
- Structured output has one format. Existing top-level decision fields are removed rather than retained as aliases.
- Hook configuration accepts `timeout_ms` only. The old `timeout` field is rejected with a diagnostic that names the replacement.
- Plugin hooks are outside the scope.
- Features without a real lifecycle source are not simulated. In particular, there is no session heartbeat timer.
- Sequential execution, exact tool-name matchers, bounded output, the four-state internal verdict model, global/project layering, and fail-closed blocking timeouts remain in place.
- Cancellation is not a policy decision. Cancelling a turn or session stops the hook process and propagates cancellation without manufacturing a denial.

## Target Contract

### Configuration

`hooks.json` continues to accept either a top-level array or an object containing `hooks`. Each declaration has this shape:

```json
{
  "event": "PreToolUse",
  "matcher": "write,edit",
  "command": "check-change",
  "timeout_ms": 30000
}
```

- `event` and `command` are required.
- `matcher` remains an exact comma-separated tool-name list, with `*` matching every tool. It remains invalid on events that do not carry a tool name.
- `timeout_ms` is an integer from 1 through 300000. The default remains 30000.
- `timeout` is invalid; it is not interpreted in either seconds or milliseconds.
- An unknown field invalidates only that declaration, not the complete file. This catches misspelled security settings while preserving independently valid hooks.
- Global declarations run before project declarations. Identical declarations are not silently removed because two layers may intentionally invoke the same command; an exact duplicate produces a warning that names both sources.

### Structured output

A hook that needs to make a decision or return explicitly marked context writes one JSON document to standard output:

```json
{
  "hook_output": {
    "decision": "deny",
    "reason": "The requested operation is outside the approved path.",
    "context": "Optional bounded context for the model."
  }
}
```

- `hook_output` is the sole structured-output envelope.
- `decision` is optional and accepts `allow`, `ask`, or `deny`; absence means `Abstain` internally.
- `reason` and `context` are optional strings. Empty strings are treated as absent.
- Standard output is either one JSON document or plain text. Commands write diagnostic logging to standard error; mixed log lines followed by JSON are not parsed as structured output.
- JSON-shaped output with an invalid envelope, an unknown decision, or a wrong field type is a hook fault. On a blocking event it refuses the action rather than silently discarding a guard decision; on an observational event it is reported and ignored.
- Plain-text output from a successful hook remains model context where that event supports context. Existing size caps apply to both plain and structured context.
- Exit code 2 remains an unconditional denial for a blocking event. Other non-zero exits remain faults, not authored decisions.

### Blocking semantics

| Event | `allow` | `ask` | `deny` | Timeout |
| --- | --- | --- | --- | --- |
| `PreToolUse` | contributes `Allow` to the permission chain | requests the existing approval flow | refuses the tool call | refuses the tool call |
| `UserPromptSubmit` | submits the prompt | invalid for this event and refuses with a diagnostic | returns a submission error before transcript or model mutation | refuses the submission |
| every other event | decision is reported and ignored | decision is reported and ignored | decision is reported and ignored | event fault only |

For multiple blocking hooks, declaration order remains authoritative. The first denial stops later hooks; otherwise `Ask` is stricter than `Allow`, and no decision remains `Abstain`. A denied prompt does not inject context produced by earlier hooks.

### Lifecycle coverage

The implementation adds or activates only events with an existing source:

- `TurnStarted` maps directly from `AgentEvent::TurnStart`.
- `UserPromptQueued` fires after a steering or follow-up message has been accepted into its queue.
- `TaskStarted` fires once after task registration has succeeded and before work begins, for foreground and background tasks.
- `SubagentStart` fires immediately before a child run begins; `SubagentStop` fires on success, failure, or cancellation. Foreground and background children use the same boundary.
- `SubagentStart` and `SubagentStop` cease being marked as accepted-but-unemitted.
- No `SessionHeartbeat` event or timer is introduced.

`TaskStarted` is the generic task signal and `SubagentStart` is the child-agent-specific signal. A subagent therefore emits both, deliberately, with different payloads.

## Implementation Plan

- [x] 1. Make the event model describe capabilities instead of one hard-coded blocking constant. In `crates/notagent/src/core/hooks/events.rs`, replace `BLOCKING_EVENT` with an event method that identifies `PreToolUse` and `UserPromptSubmit` as blocking, add `TurnStarted`, `UserPromptQueued`, and `TaskStarted`, and remove the unemitted marker from the two subagent events. Keep matcher support restricted to the existing tool-scoped events. Unit tests must enumerate the complete event set and pin blocking, matcher, and emission capabilities separately.

- [x] 2. Make hook declarations strict and unit-safe in `crates/notagent/src/core/hooks.rs`. Change the stored timeout to an integer millisecond type, read only `timeout_ms`, enforce the 1–300000 range, and reject a declaration containing `timeout` or any other unknown field while continuing to load valid siblings. Preserve both supported top-level JSON shapes and the global-before-project order. Add a second validation pass that reports exact duplicate declarations without removing them.

- [x] 3. Replace the permissive decision parser in `crates/notagent/src/core/hooks/runner.rs` with typed deserialization of the `hook_output` envelope. Parse the complete trimmed standard output when it is JSON-shaped; do not inspect only the final line and do not accept top-level aliases. Return structured context separately from plain output so the dispatcher never has to infer whether JSON should be shown to the model. Invalid structured output must carry a precise fault describing the field and expected shape, without echoing arbitrary full hook output into the UI.

- [x] 4. Separate process outcome from policy outcome in the runner. Represent successful exit, authored denial, timeout, cancellation, spawn failure, and other non-zero exit distinctly. Keep timeout as a denial only when the event is block-capable; let cancellation propagate as cancellation; and keep other execution faults visible without turning them into authored permission choices. Update sequential combination so a denial still short-circuits while prior reports remain available.

- [x] 5. Generalise `HookRuntime::decide` into an event-aware blocking path while retaining the simple observational `emit` path. The blocking result should contain the final verdict, the deciding declaration, bounded context, run reports, and cancellation state. Reject calls that try to use the decision path for an observational event, so blocking semantics cannot spread through ad-hoc call sites.

- [x] 6. Change `HookDispatcher::before_agent_start` into a prompt-submission decision that returns either accepted bounded context or a refusal reason. In `AgentSession::prompt`, run it before appending the user message, pending context, or any transcript entry. A refusal returns `Err`, reports preflight failure, starts no model request, and allows the interactive caller to keep or restore the submitted text. `Ask` is rejected for this event rather than opening a permission dialog for the user's own prompt. Do not alter `agent_settled`; `Stop` remains a final notification.

- [x] 7. Attribute injected context per hook rather than wrapping every output in one anonymous block. Render each accepted context value with its event and stable declaration order, then place the combined bounded text in the existing hidden custom message. Do not expose command strings or absolute configuration paths to the model. Keep structured decision JSON out of model context unless its `context` field explicitly supplies text.

- [x] 8. Carry `tool_call_id` through the whole tool boundary. Introduce a small request struct for the permission gate and hook decision callback containing id, tool name, and validated arguments. Pass the existing id from `BeforeToolCallContext`; for lent calls, allocate the id before permission evaluation and reuse it for execution. Add the id to `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `PermissionRequest`, and `PermissionResult` payloads without removing `tool_name`, `tool_input`, or the bounded response summary.

- [x] 9. Wire the directly available agent and queue events. Dispatch `TurnStarted` from `AgentSession::dispatch_hooks` when it receives `AgentEvent::TurnStart`, with a session-local monotonically increasing turn number. Emit `UserPromptQueued` only after `queue_steer` or `queue_follow_up` succeeds, including queue kind, prompt, image count, and resulting queue length. These are observational and must not alter queue ordering or turn timing beyond the existing awaited sequential hook contract.

- [x] 10. Add explicit task and subagent observers rather than piggybacking on UI lifecycle callbacks. `TaskStarted` fires exactly once when `TaskManager::register` has accepted a task and before its work begins, regardless of whether it starts foreground or detached; its payload carries task id, kind, description, and detached state. Wrap the shared delegation execution boundary so `SubagentStart` and `SubagentStop` cover foreground and background runs and every terminal result. Their payloads carry the child session id, optional task id, agent type, prompt or bounded result, detached state, status, and duration. Tests must prove that moving a foreground task to the background does not emit a second start.

- [x] 11. Terminate the complete process tree on timeout or cancellation. On Unix, start the shell in its own process group and signal the group with TERM followed by KILL after the existing two-second grace period. On Windows, assign the child to a job object and use the job for cancellation and timeout. After an ordinary shell exit, preserve deliberately detached descendants whose streams are redirected; terminate the owned tree only if inherited capture pipes remain open past the reader grace period. Keep ownership handles until stdout and stderr readers finish, and never signal a process group that the runner did not create. Use the existing workspace platform dependencies rather than shelling out to a second process-killing command.

- [x] 12. Preserve and pin the current resource boundaries: sequential declaration order, the 4000-character stdout/stderr cap, the 2000-character tool-response cap, the five-minute timeout ceiling, exact tool-name matchers, and global/project composition. Add tests around multibyte truncation and multiple hooks so later lifecycle work cannot accidentally change these properties while refactoring the result types.

- [x] 13. Replace provenance-style comments in files touched by this work with invariant-based explanations. In particular, comments around hook dispatch in `agent_session.rs` should explain ordering relative to permission evaluation, image normalization, retries, queues, and transcript mutation without describing where an earlier implementation emitted an event. Keep all new symbols, comments, diagnostics, and commit prose in English.

- [x] 14. Document the complete public contract in the root `README.md`: both `hooks.json` shapes, declaration fields, timeout units and limits, event payloads, blocking semantics, exact matcher behavior, output caps, exit codes, the structured-output envelope, and one safe example each for prompt validation and post-tool observation. State plainly that the contract is breaking: `timeout` and top-level decision fields are invalid.

- [x] 15. Rework the hook test suite around the contract rather than individual parser helpers. Extend `crates/notagent/tests/hooks.rs`, `hook_dispatch.rs`, and the permission integration tests to cover strict configuration, absence of legacy parsing, structured context, sequential verdict precedence, prompt refusal before mutation, cancellation versus timeout, tool-call correlation, duplicate diagnostics, and all newly wired lifecycle events. Add platform-gated process-tree tests whose child starts a grandchild and prove both disappear after timeout and cancellation.

## Verification Criteria

- A declaration with `timeout_ms: 30000` loads with a thirty-second deadline; a declaration containing `timeout` does not load and reports `timeout_ms` as the required field.
- Top-level `decision`, `permission`, or similarly shaped JSON never influences a verdict. Only `hook_output.decision` does.
- Invalid JSON-shaped output from a blocking hook cannot silently permit the guarded action.
- A `UserPromptSubmit` denial leaves the model uncalled, appends no user or hook-context message, reports preflight failure, and returns the denial reason.
- `Stop` output cannot enqueue a continuation or cause another model request.
- A timed-out blocking hook denies; cancellation stops the process tree and is not reported as denial.
- Hooks still execute in declaration order, and the first denial prevents later blocking hooks from running.
- Every tool lifecycle payload carries the same `tool_call_id` from pre-use through post-use or failure and through any permission request and result.
- One real agent turn emits one `TurnStarted`; one accepted queued message emits one `UserPromptQueued`; one accepted task emits one `TaskStarted` before its work begins.
- Every child run emits exactly one `SubagentStart` and one `SubagentStop`, including failure and cancellation. Detaching an already-running child emits neither event a second time.
- Hook stdout, stderr, structured context, and tool-response summaries remain within their current caps for ASCII and multibyte input.
- Timeout or cancellation removes a hook's shell and its descendants on Unix and Windows; ordinary exit preserves detached descendants with redirected streams.
- No plugin-owned hook path, regex matcher, parallel dispatcher, session heartbeat, or stop continuation exists after the change.
- `cargo check -p notagent`, the focused hook and permission tests, `git diff --check`, and `scripts/check.sh` are green.
- The changed source, comments, documentation, staged diff, and eventual commit prose contain no external implementation attribution.

## Risks and Mitigations

1. **The breaking parser can disable an existing security hook.** A declaration using `timeout` or a command producing old decision JSON will no longer load or decide.
   Mitigation: reject declarations individually with a precise startup diagnostic, treat invalid JSON-shaped output on blocking events as refusal, and document both breaking shapes with before/after examples. Do not silently reinterpret either format.

2. **Prompt refusal can lose text the user just typed.** The editor currently hands the text to `AgentSession::prompt` before the hook runs.
   Mitigation: make refusal an ordinary submission error before transcript mutation and add an interactive integration test that the editor retains or restores the exact submitted text.

3. **Task and subagent callbacks can double-fire across foreground-to-background transitions.** The task manager currently has lifecycle notifications tied to visibility as background work, not solely to execution start.
   Mitigation: introduce a separate once-only execution observer at registration and keep UI visibility callbacks unchanged. Pin foreground, initially detached, and later-detached cases separately.

4. **Awaited observational hooks add latency to every turn and queue operation.** More lifecycle coverage means more places where the existing sequential contract is visible.
   Mitigation: retain the deterministic contract deliberately, report hook durations, and avoid adding events without a concrete consumer. Do not hide latency by changing dispatch to fire-and-forget in the same change.

5. **Process-group termination can kill an unrelated process if ownership is inferred only from a reused id.** This is especially risky after a child exits quickly.
   Mitigation: create and retain an owned group or job handle at spawn time, signal only that owned object, and cover fast-exit plus timeout races in platform-gated tests.

6. **Adding tool-call identity through the permission chain broadens several internal APIs.** Lent calls currently create their id only after permission succeeds.
   Mitigation: introduce one request struct rather than another positional argument, generate the lent-call id before the gate, and assert that the same id reaches the executor and every hook payload.

## Explicitly Rejected Alternatives

- Retaining top-level decision aliases beside the new envelope. Rejected because two accepted contracts create permanent ambiguity and make malformed guard output look successful.
- Reinterpreting `timeout` as seconds. Rejected because an existing numeric value would silently change meaning by three orders of magnitude.
- Allowing a `Stop` hook to continue the agent. Rejected because an observational extension must not unexpectedly create another model request after the run has settled.
- Running hooks in parallel. Rejected because declarations may mutate the same workspace and declaration order is part of the observable contract.
- Regex matchers. Rejected because tool names are a closed, exact namespace and an overly broad pattern can widen the actions a hook touches.
- A recurring session heartbeat. Rejected because it creates background processes without a user action or state transition to justify them.
- Synthesising lifecycle events before the owning subsystem has a real transition to expose. Rejected because a plausible but false event is worse than an explicitly unsupported one.
