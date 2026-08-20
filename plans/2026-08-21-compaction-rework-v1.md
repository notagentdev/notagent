# Rebuilding Context Compaction

## Objective

Replace what compaction keeps and how it is written, so that a compacted session carries the user's own words forward verbatim instead of a paraphrase of them.

Today a compaction keeps the newest stretch of raw transcript and summarises everything older into a fixed seven-section template. Measured on the real session in `crates/notagent/tests/fixtures/before-compaction.jsonl` — 990 messages, 376,758 estimated tokens — that split is badly aimed:

| what | tokens | share |
| --- | --- | --- |
| tool results | 216,268 | 57.4 % |
| assistant text, thinking and tool calls | 144,999 | 38.5 % |
| bash output | 12,882 | 3.4 % |
| user messages | 2,609 | 0.7 % |

The retained window is 20,019 tokens (the last 57 entries, `crates/notagent/src/core/compaction/compaction.rs:394`), and it is spent almost entirely on tool results — file contents that have since been overwritten. Every one of the 55 user messages, 2,609 tokens in total, falls on the far side of the cut. After a compaction the agent's only record of what was asked is prose it wrote about itself, and on the next compaction it summarises that prose again (`compaction.rs:503`), so each generation drifts further from the original request while reading as though it were fact.

The new shape inverts that. The user's messages are the cheapest thing in the session and the most expensive to lose, so they are kept in full; everything else is summarised into a single note that sits last in the context. On the measured session that is roughly 2.6k of user text plus the note plus fixed request overhead — about 15k where today's mechanism leaves about 35k, while keeping strictly more of what the next turn actually needs.

Assumptions where the request was open:

- Genuine user input means `AgentMessage::User` and nothing else. Injected content already travels as `AgentMessage::Custom` (`crates/notagent/src/core/todos/reminder.rs:151`), so no origin taxonomy has to be invented; the todo reminder is re-injected live each turn and must not be preserved through a compaction.
- The retained-user-message budget is 20,000 tokens, with the oldest 2,000 reserved for the head of the session. On the measured session the budget never binds, so it only matters for sessions far longer than the fixture.
- The note is written in the language the conversation has been using. Commit messages and code comments stay English; a summary is neither, and a German session whose handoff note is English forces a translation step on every future turn.
- No other implementation is named anywhere in the code, the comments, or this plan. The reasoning is recorded on its own merits so that a reader of the code never has to go looking for an external reference.

## Implementation Plan

- [ ] 1. Establish which entries survive a compaction, as a single predicate with its reasoning written down. Only entries whose context message is `AgentMessage::User` qualify; assistant turns, tool results, bash executions, custom messages and prior compaction summaries do not. The distinction already exists structurally — `session_entry_to_context_messages` (`crates/notagent/src/core/session_manager.rs:700`) maps injected content to `AgentMessage::Custom` and only real user input to `AgentMessage::User` — so this is a matter of naming the rule and covering it, not of adding metadata. Place it beside the existing cut-point helpers in `crates/notagent/src/core/compaction/compaction.rs:309` so that the two notions of "what matters" sit together.

- [ ] 2. Select the retained user messages against a token budget, keeping both ends of the session. Walk backwards from the newest accumulating estimated size until the budget minus the head reservation is spent, then fill the head reservation from the oldest forward. A message that only partly fits is truncated — from its end when it lands in the head, from its start when it lands in the tail — so the budget is met exactly rather than by dropping a whole message. Keeping a head at all is the point: the first message of a session is usually where the actual goal is stated, and a pure tail loses it precisely in the long sessions where compaction matters most. Reuse `estimate_tokens` (`compaction.rs:278`) so the selection and the trigger agree on what a token is.

- [ ] 3. Insert a marker between head and tail whenever the selection elided anything, stating how many tokens were dropped and that the summary at the end covers them. Without it the model sees the session's opening followed immediately by recent work and reads the two as adjacent, inventing a continuity that does not exist. Omit the marker entirely when nothing was elided, so a short session carries no note about an omission that never happened.

- [ ] 4. Persist the selection in the compaction entry rather than recomputing it on load. `CompactionEntry` (`crates/notagent/src/core/session_manager.rs:96`) currently records only `first_kept_entry_id`, which is enough for a suffix but not for a head-and-tail selection with truncated boundary messages. Add the retained entry ids and any applied truncation as new optional fields, and keep `first_kept_entry_id` populated: the branch cache (`session_manager.rs:520`) and `crates/notagent-session-sqlite` both read it, and a session file written by this version must still open in a build that predates it. Recomputing instead would let a changed estimator silently rewrite the history of an old session.

- [ ] 5. Rebuild the context assembly so the summary sits last. `build_context_entries_indexed` (`crates/notagent/src/core/session_manager.rs:735`) currently emits the compaction entry followed by everything from `first_kept_entry_id` onward; it must instead emit the retained user entries in order, then the elision marker if there is one, then the summary. Only the context ordering changes — the transcript is rendered from the entry tree and stays chronological, so the compaction block continues to appear where it happened. State that separation in the code, because the two orderings diverging is exactly the kind of thing a later reader will assume is a bug.

- [ ] 6. Replace the summarisation prompt with a first-person handoff note. The current prompt (`compaction.rs:470`) mandates seven headings, and a section with nothing to put in it gets filled anyway — that is where the invented content comes from. The replacement asks for a note the agent writes to itself, in free form, whose shape follows the task: what the current request is asking for and which ambiguities are already resolved, the constraints in force with settled decisions kept separate from open questions, what was actually done at high fidelity including exact commands, paths and their results, what remains unknown and must be checked rather than assumed, and the forward plan with the concrete next call. Instruct it to be honest about anything claimed but unverified, and to write in the language the conversation has been using. Keep `SUMMARIZATION_SYSTEM_PROMPT` (`crates/notagent/src/core/compaction/utils.rs:193`), whose job — stop the model answering the conversation it was handed — is unchanged.

- [ ] 7. Delete the iterative update path. `UPDATE_SUMMARIZATION_PROMPT` (`compaction.rs:503`) and the `previous_summary` plumbing through `prepare_compaction` and `generate_summary_with_usage` exist to fold a new window into an old summary; with the whole surviving history summarised afresh each time there is no old summary to fold, and the compounding drift it causes is one of the two defects this rework targets. Removing it also removes the reason `CompactionPreparation` carries `previous_summary`.

- [ ] 8. Delete the split-turn path. `TURN_PREFIX_SUMMARIZATION_PROMPT` (`compaction.rs:542`), `generate_turn_prefix_summary`, the `turn_prefix_messages` field and the `is_split_turn` branching in `find_cut_point` (`compaction.rs:452`) all exist to explain a retained suffix that begins mid-thought. Nothing is retained mid-thought any more — the retained messages are whole user messages — so the entire second LLM call and the `combine_usage` bookkeeping around it go away.

- [ ] 9. Size the summarisation request before sending it, and cut once. The request is the surviving history plus the instruction; when its estimate exceeds what the model will accept, drop the oldest messages in a single pass until it fits, and report in the result how much was dropped. Reacting to a provider rejection instead is the wrong shape twice over: shrinking by a fixed ratio throws away far more than necessary, and dropping one message per rejection needs hundreds of round trips — measured on the fixture, reaching a 190k target from 376,758 tokens takes 440 single drops for 441 requests, ending at 189,895 tokens, against a ratio-based pass that reaches 188,179 in two. The two land within 1,716 tokens of each other, so the round trips buy nothing. Keep a rejection path as a fallback for when the estimate is wrong, but do not make it the mechanism.

- [ ] 10. Keep the existing per-tool-result truncation as the primary cost control. `truncate_for_summary` (`crates/notagent/src/core/compaction/utils.rs:108`) caps each tool result at 2,000 characters when the conversation is serialised, which on the fixture brings a 376,758-token history to roughly 227,577 tokens in the request — the difference is almost entirely tool-result tails that no summary needs. This is cheaper and less lossy than discarding whole messages, so it must run before the cut in task 9 rather than instead of it.

- [ ] 11. Replace the flat file lists with a mechanically derived operation outline. `format_file_operations` (`crates/notagent/src/core/compaction/utils.rs:82`) emits two unordered path lists; an outline that names what happened to each path — read, written, patched — and keeps only the last operation per path is strictly more informative at similar cost. It is derived from the recorded tool calls by `extract_file_ops_from_message` (`utils.rs:30`), so unlike the note itself it cannot invent a path. Measured on the fixture, deduplicating by path takes 457 operations down to 259; the file operations among them cost about 5.9k tokens, but shell commands rendered in full cost 10.4k, so commands must be reduced to their first line and the whole outline capped, with the cap stated in the output when it bites.

- [ ] 12. Simplify the settings to match the new mechanism. `keep_recent_tokens` (`compaction.rs:148`) names a quantity that no longer exists, since what is retained is now chosen by user-message budget rather than by transcript window. Decide whether it becomes the retained-user-message budget or is retired in favour of a new setting, and migrate `ResolvedCompactionSettings` (`crates/notagent/src/core/settings_manager.rs`) accordingly — a stale setting that silently does nothing is worse than one that was removed. `reserve_tokens` keeps both its jobs: the trigger threshold in `should_compact` (`compaction.rs:245`) and the summary's output budget.

- [ ] 13. Report the size change where the user already looks. `run_compaction` (`crates/notagent/src/core/agent_session.rs:3495`) already computes `estimated_tokens_after` from the rebuilt context, and the transcript block (`crates/notagent/src/modes/interactive/components/compaction_summary_message.rs:85`) already shows the before figure — it needs the after figure beside it in both the collapsed and expanded forms. This is what makes the rework verifiable in ordinary use rather than only in tests: on the fixture the line should move from roughly 377k → 35k to roughly 377k → 15k.

- [ ] 14. Keep sessions written by earlier versions readable. An entry that carries only `first_kept_entry_id` and no retained-selection fields must still rebuild into a context, using the old suffix rule, and must not be rewritten in place. Cover it with a fixture so the rule is pinned rather than assumed; `crates/notagent/tests/fixtures/before-compaction.jsonl` already contains two compaction entries in the old shape and can serve directly.

- [ ] 15. Rework the test suite around the new behaviour. `crates/notagent/tests/compaction.rs` and `crates/notagent/tests/agent_session_compaction.rs` are built around cut points, split turns and the update prompt, most of which cease to exist. The cases worth having: every user message survives a compaction when the budget allows; the budget is honoured exactly when it does not, with head and tail both present and the elision marker between them; a truncated boundary message keeps its most relevant end; the summary is last in the context and the transcript order is unchanged; the request is cut once to fit rather than retried; the operation outline keeps only the last operation per path and caps long commands; an old-shape entry still loads. Assert the retained user text verbatim rather than by token count, since verbatim retention is the whole claim.

## Verification Criteria

- After a compaction of the fixture session, every one of the 55 user messages is present in the rebuilt context, byte for byte, and the summary is the final message.
- The rebuilt context for the fixture measures under 20,000 tokens by `estimate_tokens`, against roughly 35,000 for the current mechanism on the same input.
- With a synthetic session whose user messages exceed 20,000 tokens, the selection lands within the budget, retains at least 2,000 tokens from the oldest messages, and carries exactly one elision marker naming a non-zero dropped-token count.
- A session with no elision carries no elision marker.
- The summarisation request is issued once for a history that fits, and once more at most for one that does not, with the dropped amount reported in the result.
- A compaction entry written before this change still rebuilds into a context under the old suffix rule, and the session file is left unmodified.
- The transcript block shows both the before and the after token figure, collapsed and expanded.
- No occurrence of the removed prompts, `is_split_turn`, `turn_prefix_messages` or `previous_summary` remains in the crate.
- `cargo clippy -p notagent --lib --tests` is clean and no new `unwrap()`/`expect()` sits on a path reachable from a malformed session entry, a missing model, or an empty selection.

## Potential Risks and Mitigations

1. **Dropping all assistant and tool messages removes the tail of the work in progress.** A compaction that lands mid-task leaves the next turn with the request and a note, but not the last tool result it was reading.
   Mitigation: the note is written by the same agent immediately before the drop and is instructed to record the concrete next call and the results it must not lose. Verify against a session compacted mid-task rather than only between turns, and if the tail proves too thin, retain the final assistant turn alongside the user messages before widening anything else.

2. **A summary that is always regenerated from the full surviving history costs more than one that updates incrementally.** Every compaction now re-reads everything.
   Mitigation: this is the cost of not compounding drift and is accepted deliberately. The per-tool-result truncation in task 10 keeps the request bounded; measure the request size across several consecutive compactions of one session and confirm it does not grow without limit.

3. **Free-form output is harder to test than a fixed template.** Assertions on section headings are no longer available.
   Mitigation: test the mechanism, not the prose — retention, ordering, budget, request count and the derived outline are all deterministic. Judge the note itself by reading real compactions, which is why task 13 puts the before-and-after figure on screen.

4. **The retained-selection fields change the session file format.** A file written by this version opened by an older build would lose the head-and-tail selection.
   Mitigation: keep `first_kept_entry_id` populated and meaningful, so an older build degrades to the suffix rule rather than failing. Treat the new fields as additive and optional in both directions.

5. **Truncating a user message mid-sentence can invert its meaning.** A request that ends "…but do not touch the database" becomes an instruction to touch it if the tail is cut.
   Mitigation: truncate tail messages from the start and head messages from the end, so the most recently written text of each survives, and mark every truncation visibly in the retained text rather than splicing silently.

6. **The estimator decides the cut, and it is a heuristic.** An underestimate sends a request the provider rejects.
   Mitigation: `estimate_tokens` already rounds up per message and charges a flat 4,800 characters per image (`compaction.rs:260`), which biases it high; keep the rejection path from task 9 as the fallback and log when it fires, since it firing regularly means the estimator needs work.

## Alternative Approaches

1. **Keep the current window mechanism and only replace the prompt.** The cheapest possible change, and it would fix the invented-content problem on its own. Rejected as the whole answer because it leaves the user's messages on the wrong side of the cut and leaves the update path compounding drift — but it is a valid intermediate state if the rework has to be landed in two releases, and tasks 6, 7 and 8 form exactly that increment.

2. **Summarise mechanically, with no model call at all.** Render the evicted range as a structured outline — role blocks, text kept verbatim, tool calls as one line each, deduplicated per path. Free, deterministic, and impossible to hallucinate. Rejected as the primary mechanism: simulated on the fixture it produces 33,984 tokens, more than twice the target, because assistant prose passes through unshortened; and since the outline is itself a message, each compaction re-renders the previous one, so it grows monotonically and never shrinks. Task 11 adopts the half of it that is sound — the mechanically derived operation outline — and leaves the prose to the note.

3. **Discard everything and start a fresh context window.** No summary, no selection, a hard reset. Trivially cheap and predictable. Rejected because it moves the entire cost onto the user, who must restate the task; the measured data says the user's messages are 0.7 % of the session, so keeping them is nearly free and reconstructing them is not.

4. **Retain a fixed number of recent user messages instead of a token budget.** Simpler to reason about and to test. Rejected because user messages vary by two orders of magnitude — 7 to 540 tokens on the fixture — so a count is a poor proxy for size; a budget with a count as a secondary cap could be revisited if a single pasted message ever crowds out the rest.

5. **Store the compacted context directly rather than deriving it from entries.** The compaction entry would carry its replacement history verbatim, removing the need to re-derive a selection on load. Rejected because the session file is a tree that everything else derives from, and a second source of truth for the same context is how the live and reloaded views drift apart; task 4 persists the selection but keeps the messages themselves in the entries where they already are.
