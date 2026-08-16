---
name: create-port-plan
description: Plan a 1:1 port or migration of an existing codebase to another language or stack, executed by multiple autonomous agents in parallel. Produces a fact base extracted from the real source, a master plan with a binding tech-substitution table, per-workstream plans with checkbox tasks, self-contained agent prompts, and the coordination infrastructure (parity ledgers, interface-request channel, gates) that keeps parallel agents drift-free without user intervention. Use when the goal is behavior-preserving porting, rewriting, or migrating of a codebase that already exists — not for greenfield features (use create-plan for those). Strictly planning-focused with no code modifications.
---

# Create Port Plan

Plan a behavior-preserving port of an existing codebase, sized for several autonomous agents working in parallel git worktrees. This skill extends `create-plan` with everything a 1:1 port needs and a feature plan does not: a mandatory fact-finding phase, drift control against the original, oracle-based verification, and the coordination machinery for multi-agent execution.

Derived from a completed ~120k-LOC TypeScript→Rust port (9 crates, 3 agents, ~48k ported tests, byte-level oracles, zero unresolved parity gaps). Every rule below earned its place by catching a real defect or preventing a real stall in that project.

## When to Use

- Porting a codebase to another language (TS→Rust, Python→Go, …) with 1:1 behavior
- Large migrations/rewrites where the old implementation is the specification
- Any project where "nothing may rest on assumptions" and several agents must work in parallel without user intervention

**Not for**: greenfield features, refactors inside one language, or single-agent tasks — use `create-plan`.

## Core Principles

1. **The source code is the spec; its tests are the oracle.** Every behavior claim in the plan cites a file (and line where it matters). The original test suites are ported with the code — same cases, same expected values.
2. **Facts before plan.** No task is written before the fact base exists. A plan sentence without a source citation is an assumption, and assumptions are forbidden.
3. **Reading is not optional.** Fact sheets are maps, not substitutes. Every executing agent must fully read the source files named in its task before porting them, and log that in the ledger.
4. **Drift has exactly four doors.** Only these deviation classes are allowed, each logged in the ledger: (1) language idiom with identical observable behavior, (2) scoped exclusions listed in the master plan, (3) tech substitutions from the master table, (4) distribution mechanics. Everything else is drift and gets fixed, not discussed. Source bugs are replicated and marked `bug-compat` — never silently "improved".
5. **Autonomy needs rails, not supervision.** Agents never ask the user. Uncertainty protocol: re-read the source → run the original tests → observe behavior in the original runtime (REPL) → pin the result as a test. Blockages go to an append-only request channel with a defined owner.

## Planning Process

### Phase 0 — Fact-finding (mandatory, before any plan text)

Launch parallel deep-analysis agents (one per subsystem/package) plus your own first-hand verification of the most critical paths. Write results to `plans/facts/<topic>.md`. Each fact sheet must contain:

- Complete file list with LOC and one-line purpose per file
- Public API surface, central types with exact fields, event/message inventories
- External dependencies and what each is used for (these become the substitution table)
- Test framework, test approach, and test LOC (the tests are scope, not garnish)
- Port pitfalls found in the code (globals, in-place mutation, deliberate non-idioms)

Hunt explicitly for these — all were real findings:
- **Hidden couplings**: core features implemented via plugin/extension indirection (if you drop the plugin system, those features must be rebuilt natively — enumerate every dispatch point)
- **Dead weight**: stubs, experimental dirs, packages nothing consumes — verify via import-grep, then exclude *with evidence*
- **The actually-consumed surface**: grep what dependents really import; port that, not the theoretical API
- **Reference implementations**: existing prior-art code in the target language that can be adapted instead of re-ported

### Phase 1 — Master plan

One file, `plans/{YYYY-MM-DD}-{name}-master-v{N}.md`, validated with `validate-plan.sh`. Beyond the standard sections it must contain (see `references/port-master-plan-template.md`):

- **Scope table**: target module ← source package, LOC, owning workstream
- **Exclusion table**: every exclusion with its evidence (file:line or grep result)
- **Binding tech-substitution table**: source dep → target equivalent. This table is law; a substitution not listed is drift. A *gap* in the table is a plan bug: the agent proposes, the orchestrator ratifies and amends the table (never silently).
- **Architecture decisions** that protect observable behavior (e.g. keep the original's data representation even if unidiomatic — changing it changes diff/render behavior)
- **Workstream split** with disjoint file ownership, worktree/merge protocol, and gates
- **Drift-control rules** (deviation classes, ledger duty, oracle duty, bug-compat rule)

### Phase 2 — Workstream plans

One plan file per workstream, same validator. Task anatomy (see `references/workstream-plan-template.md`):

- Source files to read (with LOC — the number makes skipping visible), target modules, tests to port, rationale, dependencies
- **Test infrastructure first**: for UI/rendering, the virtual-terminal (or equivalent) harness is task 1 — without it nothing is acceptable
- **Contract-first tasks**: public types land as the first, immediately-merged commit so dependent workstreams build against them from day one
- Where the source has no test suite for a file: the task requires a **generated oracle** — a script that drives the *real original implementation* with fixtures and records its output; the port is tested byte-wise against that recording
- Keep e2e tests that need real credentials key-gated exactly as the original does

### Phase 3 — Agent prompts

One self-contained prompt file per agent in `plans/prompts/` (see `references/agent-prompt-template.md`). Each prompt must carry: mission, worktree setup, mandatory reading order, the iron rules (read-before-port, oracle duty, deviation classes), the work loop (task → read → port tests → implement → ledger → check → merge), ownership boundaries, the uncertainty protocol, and a definition of done. Prompts are **re-entrant**: a restarted agent finds its state in checkboxes and ledgers and continues at the first unchecked task. Write plans and prompts in the user's language.

### Phase 4 — Validate

Run `./newskill/create-port-plan/validate-plan.sh` (or the copied path) on every plan file and fix all errors. The bundled validator contains a fix the stock one lacks: `grep -q` under `set -o pipefail` SIGPIPEs on large plans and reports false failures — this version reads its input fully.

## Coordination Infrastructure (create these with the plan)

Formats in `references/coordination-files.md`.

- **`plans/interface-requests.md`** — the *only* cross-workstream channel. Append-only, ID'd entries (`A-1`, `B-1`, `C-1`, orchestrator `O-1`), each with from/to, evidence, concrete request, status. Owners implement; requesters never touch foreign files. Orchestrator entries ratify substitutions, reassign ownership, and answer escalations — every plan change is an O-entry, never a silent edit.
- **`PARITY.md` per target module/crate** — one row per source file on a status ladder (read → ported → tests ported → verified), deviations with class and justification. A task is done when its ledger rows are complete.
- **Gates** — numbered sync points with *measurable* criteria, git tags, and a written gate report. First gate is the workspace scaffold (owned by exactly one agent; everyone else starts with their reading phase, so a simultaneous start is safe). Structure the build config so adding modules never touches shared files (e.g. glob workspace members) — that removes the main merge-conflict source.
- **Parity-audit tooling** — a script that enumerates every source file and checks it has a ledger row (verified or documented exclusion), exit 0 required for the final gate. **Build it early and hand it to the first agent that finishes** — in the reference project it surfaced 22 unledgered files, including a genuine plan gap nobody had noticed.

## Orchestration Guidance (for executing the plan later)

- **Merge protocol**: rebase on main → full check (fmt + lint-as-error + all tests) → fast-forward merge. On ff failure: rebase again, retry. Never merge red.
- **Expect the app/integration workstream to be the bottleneck.** Plan the relief valve up front: when a workstream finishes, redistribute cleanly-cut modules to it via an O-entry with explicit file ownership. Watch for **micro-module unlocks** — in the reference project, 50 lines owned by the busy workstream blocked 1,100+ lines at an idle one; transferring them was the highest-leverage decision available.
- **Batch sessions over frequent restarts.** Every agent start re-reads plans and sources before producing; near the end, prefer one long directed session ("work through X, Y, Z without interim reports") and combine late gates into one acceptance pass.
- **Status = artifacts, not claims**: checkboxes, ledger rows, tags, and a full check run on main. Verify agent reports against git.

## Operational Pitfalls (each cost real time — put them in the scaffold or the prompts)

- **Build-dir disk explosion**: N worktrees × full debug info filled a 926 GB disk (total stall, agents couldn't even `rm`). Scaffold with `debug = "line-tables-only"` (backtraces keep file:line — all anyone uses) and `incremental = false`; add a housekeeping rule: check free space before test runs, clean only your *own* worktree's build dir.
- **Exit codes swallowed by pipes**: `check.sh | tail` reports tail's exit code. Two agents independently reported false greens this way — and the stock plan validator has the same bug class. Run checks unpiped where the exit code matters.
- **Polling tests + fast fakes = hangs**: a loop polling `is_running()` spins forever when the faked work finishes before the first poll. Fix the harness (throttle the fake, or event-driven waits with a deadline), never delete the test. Flaky-fix acceptance: N consecutive green runs (use 10).
- **Waiter-registration races**: register the waiter *before* checking state (tokio `Notify` needs `pin!` + `enable()` first); and wait on *this* run's completion handle, not on "is anything running" (identity compare, not occupancy check).
- **Stale test binaries** from killed runs keep running and eat CPU/locks — kill your own before checks.
- **Parallel workspace test runs in one target dir** can wedge each other — one check at a time per worktree.

## Critical Requirements

- ALWAYS run fact-finding before writing tasks; every task cites source files with LOC
- ALWAYS validate every plan file with the bundled validator
- ALWAYS use checkbox format for tasks; no code blocks in plan files (validator enforces both)
- ALWAYS create the coordination files (request channel, ledger templates, audit tool task) with the plan — they are deliverables, not afterthoughts
- Define the deviation classes and the substitution table in the master plan; treat gaps as plan bugs with a ratification path
- Make prompts re-entrant and self-contained; the user's only job is starting agents

## Boundaries

This is a **planning-only** skill: research, fact sheets, plans, prompts, coordination-file templates — no production code, no builds, no tests during planning. The Orchestration Guidance section applies to later execution turns, where the orchestrator may also perform mechanical unblocking (formatting fixes, dependency additions, ownership transfers) — always documented as O-entries.
