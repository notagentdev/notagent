# create-port-plan

Plan a 1:1 port or migration of an existing codebase to another language/stack, executed by multiple autonomous agents in parallel worktrees.

Derived from the `create-plan` skill and hardened with the lessons of a completed ~120k-LOC TypeScript→Rust port (3 agents, 9 crates, ~48k ported tests, byte-level oracles, parity audit exit 0). What it adds over `create-plan`:

- **Phase 0 fact-finding** before any task is written (parallel source analysis into `plans/facts/`, hidden-coupling and dead-code hunts, consumed-surface greps)
- **Drift control**: read-before-port duty, per-crate parity ledgers with a status ladder, exactly four allowed deviation classes, bug-compat rule
- **Tests as oracle**, including generated oracles that drive the *real* original implementation where no suite exists
- **Multi-agent machinery**: disjoint ownership, worktree/ff-merge protocol, contract-first commits, gates with tags and reports, an append-only interface-request channel with orchestrator (O-) entries
- **Parity-audit tooling** built early, exit 0 required for the final gate
- **Operational pitfalls** baked into scaffold and prompts (build-dir disk explosion, pipe-swallowed exit codes, polling-test hangs, waiter-registration races, stale test binaries)
- The bundled `validate-plan.sh` includes the SIGPIPE/pipefail fix (stock version false-fails on plans >~16 KB)

## Files

- `SKILL.md` — the skill
- `validate-plan.sh` / `validate-all-plans.sh` — plan validators (fixed)
- `references/port-master-plan-template.md` — master plan skeleton
- `references/workstream-plan-template.md` — per-workstream plan skeleton
- `references/agent-prompt-template.md` — re-entrant agent prompt skeleton
- `references/coordination-files.md` — interface-requests format, PARITY ledger, audit tool, scaffold contents

## Install

Copy the folder into a skill location, e.g. project-local:

    cp -R newskill/create-port-plan .claude/skills/create-port-plan

or user-global:

    cp -R newskill/create-port-plan ~/.claude/skills/create-port-plan
