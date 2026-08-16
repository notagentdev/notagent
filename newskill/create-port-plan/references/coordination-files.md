# Coordination Files (created together with the plan)

Three artifacts carry all cross-agent state. They are deliverables of the planning phase — the plan is not done until they exist (as files or as templates the scaffold task creates).

## 1. plans/interface-requests.md — the only cross-workstream channel

Header rules (put them verbatim at the top of the file):

- Append-only: neue Einträge unten in der eigenen Sektion anhängen, fremde Einträge nie löschen oder umschreiben. Der Owner der betroffenen Dateien setzt um und hakt ab.
- Ownership-Zeile: welche Pfade welchem Workstream gehören.
- IDs fortlaufend je Absender: `A-1`, `B-1`, `C-1`, …; Orchestrator-Einträge `O-1`, ….

Entry format:

    ### <ID> <kurzer Titel>
    - **Von / An**: <Workstream> → <Workstream>
    - **Datum**: YYYY-MM-DD
    - **Betrifft**: <Datei/Typ/Modul>
    - **Beleg**: <Quelldatei mit Zeilen, die das Verhalten festlegt>
    - **Wunsch**: <konkret, mit Signaturvorschlag>
    - **Status**: offen | umgesetzt (<commit>) | abgelehnt (<Begründung>)

Orchestrator entries (`O-…`) are how the plan changes after launch: ratifying a substitution-table gap, transferring file ownership, redistributing modules to a finished workstream, recording incident rules (disk housekeeping, flaky-test protocol). Every plan change is an O-entry — never a silent edit. Owners may reject requests that contradict source behavior, citing the source.

Proven request patterns worth encouraging in prompts:
- **Blockade report**: table of missing module → owner → what it unlocks → LOC. Enables the orchestrator to spot micro-module unlocks (tiny files blocking big downstream work).
- **Delivery notice**: "X ist auf main, damit ist bei dir Y entsperrt" after merging something another workstream waits on.
- **Counter-read**: when someone had to touch a foreign file mechanically (emergency fix), the owner re-reads it against the source and confirms or corrects.

## 2. PARITY.md — one ledger per target module/crate

One row per source file. Status ladder (a row only ever moves up):

    gelesen (LOC) → portiert → Tests portiert → verifiziert

Columns: source file | target module | status | deviations (class + one-line justification).

Rules:
- A task is done only when its ledger rows are complete.
- Excluded files get a row too, with the exclusion class and evidence — the audit tool treats "documented exclusion" as satisfied.
- Deviation classes (from the master plan): 1 idiom / 2 scoped exclusion / 3 substitution / 4 distribution; plus `bug-compat` markers for replicated source bugs.
- Sections for foreign contributions inside a shared crate (e.g. "B: model layer" inside the app crate) keep ownership auditable.

## 3. Parity-audit tool + gate reports

- `scripts/parity-audit.sh` (or a small tool): enumerate every file under the source's src trees, check each has a ledger row at `verifiziert` or a documented exclusion; write the gap report to `plans/final-parity-audit.md`; `--check` exits non-zero on gaps. Required exit 0 for the final gate. Assign building it to the first workstream that finishes its plan — early runs surface unledgered files while there is still time to port them (in the reference project: 22 findings, including a genuine plan gap).
- Gate reports: `plans/g<N>-gate-report.md` per gate — date, acceptor, criteria checklist with evidence (test counts, tag), deviations. The final gate adds a smoke report (`plans/g<final>-smoke-report.md`) covering the real-terminal checks that cannot be automated.

## Scaffold contents (gate-0 task of the designated owner)

- Workspace config with glob member discovery (adding modules must never touch shared files)
- Build profile: `debug = "line-tables-only"`, `incremental = false` (prevents the disk-explosion stall; backtraces keep file:line)
- `scripts/check.sh`: format check + lint-as-error + full test run — agents run it unpiped before every merge
- `CONVENTIONS.md`: module layout mirrors source file structure, error-handling style, ledger format, commit convention
- Empty ledger per module, `plans/interface-requests.md` with header rules
