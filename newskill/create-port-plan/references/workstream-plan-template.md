# [Port Name] Workstream [X]: [scope summary]

## Objective

[What this workstream ports (packages, LOC), what it owns exclusively, which fact sheets apply, pointer to the master plan for binding rules. State the ledger file and the read-before-port duty explicitly.]

## Implementation Plan

Task anatomy — every task must name: the source files to READ FULLY (with LOC), the target modules, the tests to port, dependencies on other tasks/workstreams, and a rationale. Order tasks so that:

- [ ] 1. [Test infrastructure first (for UI: the virtual-terminal/e2e harness with hard acceptance criteria) — nothing else is acceptable without it]
- [ ] 2. [Contract types next if other workstreams depend on them — merged immediately]
- [ ] 3. [Foundation modules everything else multiplies through (width/encoding/util layers) with their regression suites]
- [ ] 4. [… mechanical middle: one task per coherent source module cluster; where a file has NO original suite, the task requires a generated oracle against the running original]
- [ ] 5. [Public API consolidation + ledger completion as the final task: export surface mirrors the source index, every file has a ledger row]

## Verification Criteria

- [All ported suites green; per-file evidence in the ledger]
- [Oracle-based byte comparisons for the render/serialization-critical parts]
- [Ledger complete: N of N source files, deviations classified]

## Potential Risks and Mitigations

1. **[The riskiest single decision of this workstream — name it and the fallback]**
2. **[Semantic gaps between source stdlib and target crates — mitigation: regression tests first, source behavior wins, pin via original-runtime fixtures]**

## Alternative Approaches

1. **[Rejected shortcut]**: [why the 1:1 requirement rules it out]
