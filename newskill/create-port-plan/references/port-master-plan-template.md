# [Port Name] (Master Plan)

## Objective

[One paragraph: what is being ported, from where to where, what the end result is (e.g. "a single self-contained binary with the full feature scope of the original"), how many agents work in parallel, and the one-sentence drift promise: gates and parity ledgers ensure nothing deviates from source behavior. Name the fact base: "Die vollständige Faktenbasis liegt in plans/facts/ (…). Sie ersetzt nicht das Lesen der Quellen: Jeder ausführende Agent MUSS die in seinen Tasks genannten Quelldateien vollständig lesen, bevor er portiert."]

## Scope

**Ported (target module ← source package):**

| Target module | Source | src LOC | Workstream |
|---|---|---|---|
| [module] | [package] | [n] | [A/B/C] |

**Excluded (each with evidence):**

| Exclusion | Evidence |
|---|---|
| [what] | [file:line, import-grep result, or "private: true" — never a bare opinion] |
| [features hidden behind the dropped subsystem that MUST be rebuilt natively] | [dispatch-point list from facts] |

**Binding tech substitutions (any further substitution is drift; gaps in this table are plan bugs — agent proposes, orchestrator ratifies and amends):**

| Source | Target |
|---|---|
| [runtime/dep] | [equivalent + the reason when non-obvious] |

## Architecture and Concurrency

[Decisions that protect observable behavior: data representations kept as-is, concurrency mapping (what becomes a real parallel task, which event orders are contractual), scaffold rules that prevent merge conflicts (glob workspace members), build profile (line-tables-only, incremental off).]

## Parallelization: Workstreams, Worktrees, Gates

[Workstream list with plan-file links and disjoint ownership. Worktree setup, branch names, launch order (scaffold owner starts first; others begin with their reading phase). Merge protocol: rebase → full check unpiped → ff-only merge. Gates table: number, criteria (measurable), tag name, report file. Consider merging the last two gates into one acceptance pass.]

## Drift Control

[The four deviation classes. Read-before-port with ledger logging. Tests-as-oracle including generated oracles for files without suites. bug-compat rule. Gate self-audits. Parity-audit tool requirement (exit 0 before final gate).]

## Implementation Plan

- [ ] 1. [Gate 0 scaffold task — single owner, tag, what exactly it contains]
- [ ] 2. [Contract-first commits — which types, which owner, merged immediately]
- [ ] 3. [Execute workstream A per its plan file]
- [ ] 4. [Execute workstream B per its plan file]
- [ ] 5. [Execute workstream C per its plan file]
- [ ] 6. [Gate acceptances with criteria references]
- [ ] 7. [Final drift audit task: every source file has a ledger row verified or excluded; audit report file]

## Verification Criteria

- [Full check green on main at every gate]
- [Ported test volume targets per package, evidenced in ledgers]
- [The behavior-critical subsystem verified byte-comparably (name the mechanism)]
- [Concurrency claims proven by wall-clock tests]
- [Config/data compatibility: original files open, roundtrip, re-serialize losslessly]
- [Ledger completeness via audit tool, exit 0]

## Potential Risks and Mitigations

1. **[Risk]**
   - Impact / Likelihood / Mitigation / Contingency
2. **[Bottleneck workstream risk — always include: name the relief valve (redistribution via O-entries) up front]**

## Alternative Approaches

1. **[Alternative]**: [why rejected — e.g. framework X would change observable behavior, SDKs would hide the semantics the original controls deliberately]

## Assumptions

- Keine Annahmen über Quellverhalten; Unklarheiten werden durch Lesen der Quelle bzw. Ausführen der Originaltests geklärt. Einzige Vorab-Festlegungen: die dokumentierten Substitutionen und Ausschlüsse dieses Dokuments.

## Dependencies

- [Read access to the source repo and any reference implementations during the whole port]
- [Original runtime available to run tests/REPL as oracle]
- [Toolchain]

## Notes

- [Prompt file locations and launch order; the request channel file; language of plans/prompts follows the user.]
