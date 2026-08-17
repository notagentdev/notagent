# Bash Filter — Shell-Output Filtering and Compression

## Objective

Take the complete shell-output filter feature over from `../notagent-main-rust`
(`crates/notagent_output_filter`, 5075 LOC Rust plus 63 TOML filter files) into
this port, wire it into the v2 `bash` tool, and expose it as `/bash-filter
[on|off]` plus a settings-menu entry.

The feature compacts the output of supported shell commands before it enters the
agent context: 35 native post-processors (cargo, go, pytest, vitest, jest,
eslint, tsc, pnpm, pip, ruff, mypy, …), 6 filesystem-backed system adapters
(`ls`, `tree`, `find`, `grep`, `rg`, `cat`/`head`/`tail`), and 63 declarative
TOML filters (make, terraform, gradle, helm, systemctl, …). Where it helps, it
also rewrites the invocation before execution so the command emits a
machine-readable format the post-processor can compact (for example `go test`
gaining `-json`), and for `find` and the read commands it answers from the
filesystem instead of spawning a child process at all.

Expected outcome after this plan:

- A `bash_filter` module in `crates/notagent` carrying every filter, adapter and
  rewrite of the reference, with the reference test suite as the oracle.
- The `bash` tool consulting it: rewrite before execution, filter after, with
  raw output still recoverable, and background tasks behaving like foreground
  ones.
- `/bash-filter on|off`, a settings-menu entry, and a `bashFilter` setting.
- The string `rtk` present nowhere in code or comments.

### Scope of "all filters"

"All filters" in this plan means the reference's complete inventory:

- 63 TOML filters, `../notagent-main-rust/crates/notagent_output_filter/src/filters/`
- 35 native post-processors, `../notagent-main-rust/crates/notagent_output_filter/src/native.rs:14`
- 6 system adapters, `../notagent-main-rust/crates/notagent_output_filter/src/system.rs:148`

The upstream project in `../rtk-main` carries a further 47 native command
modules under `src/cmds/` (~44k LOC) that the reference itself does not have.
Those are **out of scope** here; this plan ports the reference in full, not the
upstream superset. The 63 TOML files in `../rtk-main/src/filters/` are
byte-identical to the reference's copies (verified by directory diff; only
upstream's `README.md` is extra), so the declarative half is already complete
relative to upstream.

### Naming and attribution

Per user decision (2026-08-17) the feature is called **bash filter**. The string
`rtk` must not appear in any Rust source, comment, TOML file, setting name,
command name or user-visible message. That affects, at minimum:

- every file header of the reference module, which names the upstream project
- `parse_rtk_find_args`, `../notagent-main-rust/crates/notagent_output_filter/src/system.rs:753`
- the `find` rejection message, `../notagent-main-rust/crates/notagent_output_filter/src/system.rs:692` —
  user- and model-visible, so it must be reworded, not just renamed
- `RetentionMeasurement::full_rtk_tokens`, `../notagent-main-rust/crates/notagent_output_filter/src/lib.rs:481`
- doc comments in `toml_filter.rs`, `native.rs` and `system.rs` that describe
  behaviour as "pinned rtk" or "full rtk", and the test names that echo them

Concern to note, then proceed: the upstream project is Apache-2.0, whose section
4 asks that attribution notices be retained in derivative works. Stripping every
mention from the source removes that attribution. This plan therefore keeps the
provenance in a top-level `NOTICE` file, which is not code and not a comment, so
the naming rule and the licence obligation are both satisfied. If the user
prefers no attribution anywhere at all, that is their call to make explicitly.

### Assumptions

- **Default off.** The reference defaults `shell_output_filter.mode` to `Off`
  (`../notagent-main-rust/crates/notagent_config/src/config.rs:26`) and the
  `atomic_leases` port addition (v0.1.19) established the same convention here.
  This plan assumes off by default; flip in task 12 if the user wants otherwise.
- **Version.** The work lands as v0.1.20 (current workspace version is 0.1.19,
  `Cargo.toml:6`), following the one-feature-per-version practice of this repo.
- **Module, not crate.** The reference is a separate crate because its workspace
  has 45 of them. This port keeps one crate per TS package
  (`CONVENTIONS.md`, section 1), and the filter has no TS counterpart, so it
  becomes a module of `crates/notagent` next to the other port additions
  (`find_codebase`, `file_lease`) rather than a new workspace member.

## Background: what has to come across

### The reference's shape

Five source files plus a build script and 63 data files:

| Reference file | LOC | What it holds |
|---|---|---|
| `src/lib.rs` | 938 | Public API: `prepare`, `filter`, `filter_with_cwd`, `execute_override`, `should_retry_original`, `estimate_tokens`, `measure_retention`, `builtin_filter_count`; eligibility rules; invocation rewrites; the never-worse guard |
| `src/command.rs` | 522 | Quote-aware shell lexer, classification into leading command plus suffix, substitution/heredoc/redirect rejection, wrapper unwrapping (`npx`, `pnpm exec`, …), normalisation |
| `src/native.rs` | 1434 | 35 native filters, command detection, content-based detection, the post-processors themselves |
| `src/system.rs` | 1665 | 6 system adapters: `ls`/`tree` rewrite plus compaction, `grep`/`rg` rewrite plus grouping, `find` and read executed against the filesystem, glob matcher, search-line truncation |
| `src/toml_filter.rs` | 454 | Eight-stage declarative pipeline (strip ANSI, replace, match_output, line filter, truncate, head/tail, max_lines, on_empty) and the registry |
| `build.rs` | 62 | Concatenates the 63 TOML files into one embedded document, asserts count and uniqueness |
| `src/filters/*.toml` | 2402 | 63 filters and their 154 inline fixtures |

The integration side lives in
`../notagent-main-rust/crates/notagent_app/src/tool_executor.rs`:
`background_execution_command` at line 178, `finalize_shell_output` at line 189,
`shell_recovery_contents` at line 147, and the streaming path at line 774.

### The three execution modes

The feature is not one function but three different relationships to the child
process, and every one of them has to be reproduced:

1. **Filter only.** Command runs verbatim, output is compacted afterwards. All
   TOML filters and most native ones.
2. **Rewrite then filter.** The invocation is changed so the tool emits a
   parseable format, then the output is compacted (`go test` → `go test -json`,
   `pytest` → `pytest --tb=short -q -rxX`, `ls` → `env -u CLICOLOR_FORCE -u
   FORCE_COLOR LC_ALL=C ls -la`, `rg` → `NO_COLOR=1 rg -n --with-filename
   --null`). The *original* command stays what the user and the transcript see.
3. **Execution override.** `find`, `cat`, `head` and `tail` are answered from
   the filesystem by the adapter; no child process is spawned at all
   (`../notagent-main-rust/crates/notagent_output_filter/src/system.rs:177`).

Plus two safety valves: `buffers_live_output` holds back live streaming for
the system filters whose shape the adapter dictates, and `should_retry_original`
re-runs an unparseable rewritten search verbatim.

### Where the v2 side differs from the reference

Four differences are structural and drive the tasks below.

- **The streams are merged.** The reference filters `stdout` and `stderr`
  separately. The v2 bash tool interleaves both into a single byte sink:
  `ReaderEvent::Chunk` carries no stream tag
  (`crates/notagent/src/core/tools/bash.rs:201`) and `BashExecOptions::on_data`
  receives one undifferentiated stream
  (`crates/notagent/src/core/tools/bash.rs:122`). That is inherited from the TS
  original and is not worth unpicking for this feature.
- **Operations are pluggable.** `BashOperations`
  (`crates/notagent/src/core/tools/bash.rs:143`) lets a caller run commands on
  another machine. The reference has no such seam, so it never had to ask
  whether a filesystem-backed override is even addressing the right filesystem.
- **A command prefix may be prepended.** `command_prefix` produces
  `{prefix}\n{command}` (`crates/notagent/src/core/tools/bash.rs:1390`), and the
  classifier refuses multi-line input
  (`../notagent-main-rust/crates/notagent_output_filter/src/command.rs:65`).
- **Recovery already exists.** v2 spills long output to a temp file and names it
  in a `[… Full output: <path>]` notice
  (`crates/notagent/src/core/tools/bash.rs:813`), where the reference writes
  separate recovery files per stream.

## Implementation Plan

- [x] 1. **Add the module skeleton and its dependencies.** Create
  `crates/notagent/src/core/bash_filter.rs` as the module root with
  `command`, `native`, `system` and `toml_filter` submodules under
  `crates/notagent/src/core/bash_filter/`, and register the module in
  `crates/notagent/src/core.rs`. Add `toml` to `[workspace.dependencies]` in
  `Cargo.toml` and to `crates/notagent/Cargo.toml`; it is the only new
  dependency, `regex`, `serde`, `serde_json` and `ignore` are already there
  (`crates/notagent/Cargo.toml:20`). Per `CONVENTIONS.md` section 5 the version
  goes only into the root manifest and the crate refers to it with
  `workspace = true`. Rationale: everything else in the plan needs a place to
  land, and getting the dependency and module wiring wrong at the end is far
  more expensive than getting it right first.

- [x] 2. **Copy the 63 TOML filter files and embed them without a build
  script.** Copy every file from
  `../notagent-main-rust/crates/notagent_output_filter/src/filters/` into
  `crates/notagent/src/core/bash_filter/filters/` byte for byte — they are data,
  not code, and re-deriving them would be exactly the kind of simplification
  this plan forbids. The reference concatenates them at build time with a build
  script that asserts the count and the uniqueness of names
  (`../notagent-main-rust/crates/notagent_output_filter/build.rs:27`). This
  repository has no build script anywhere, and its established way of embedding
  data files is an explicit `include_str!` list, as `crates/notagent/src/core/modes.rs:42`
  does for the built-in modes. Follow that: an explicit, alphabetically ordered
  list of 63 entries, concatenated at first use behind a `OnceLock`, with the
  build script's three assertions (63 files, 63 filter definitions, no duplicate
  names) becoming unit tests instead of build-time panics. Rationale: the
  guarantees survive, the repository gains no build step, and a missing file
  fails a test rather than a compile.

- [x] 3. **Port the TOML filter engine.** Bring
  `../notagent-main-rust/crates/notagent_output_filter/src/toml_filter.rs` over
  in full: the deserialisation types with `deny_unknown_fields`, the schema
  version check, filter compilation including the mutual exclusion of
  `strip_lines_matching` and `keep_lines_matching`, the `OnceLock` registry,
  first-match lookup by command regex, and the eight-stage `apply` pipeline in
  its exact order — strip ANSI, replace, match_output with its `unless` guard,
  line filter, `truncate_lines_at`, the head/tail combination, `max_lines`,
  `on_empty` — together with the `PipelineLossiness` result each stage
  contributes to. The stage order and the exact wording of the interposed
  markers — the omitted-lines and truncated-lines notes — are what the 154
  inline fixtures assert, so neither may drift. Rationale: this is the half of
  the feature that covers the long tail of tools, and it is pure data-driven
  logic with no integration surface, so it can be finished and verified on its
  own.

- [x] 4. **Port the command classifier.** Bring
  `../notagent-main-rust/crates/notagent_output_filter/src/command.rs` over in
  full: the quote-aware lexer with its token kinds (word, pipe, operator,
  redirect, shellism), the rejection of multi-line input, command substitution,
  heredocs and file-target redirects, the distinction between a descriptor
  duplication (`2>&1`, which keeps the output reachable) and a file redirect
  (which does not), the `/dev/null` discard exception, `sudo` rejection, the
  split into leading command plus verbatim suffix, the `compound` flag, wrapper
  unwrapping for `npx`/`bunx`/`pnpm exec`/`npm exec`/`yarn exec`, environment
  assignment detection, basename normalisation, and `append_after`. Rationale:
  every filter decision downstream is made from this classification, and its
  conservatism is the safety property of the whole feature — a command it
  misreads is a command whose output the agent may silently lose.

- [x] 5. **Port the native filters.** Bring
  `../notagent-main-rust/crates/notagent_output_filter/src/native.rs` over in
  full: the `NativeFilter` enum with all 35 variants and its three predicates
  (`is_system`, `is_search`, `uses_execution_override`), command-based
  detection including its subcommand and wrapper rules, content-based detection
  for output that reached the filter through a script or a `make` target, the
  golangci JSON-compatibility probe, the `stdout_only` set that decides whether
  stderr survives, and every one of the post-processor functions with their
  exact output wording and their caps (diff at 500 lines and 100 shown changes,
  log at 50 lines truncated to 120 columns, cargo diagnostics at 40, clippy at
  60, pnpm list at 100, eslint at 20 files, go failures at 20, pytest failures
  at 30, ruff at 50, pip at 100, uv at 40). Rationale: these strings are what
  the model reads; a reworded summary is a behaviour change, and the caps are
  the difference between compaction and a second flood.

- [x] 6. **Port the system adapters.** Bring
  `../notagent-main-rust/crates/notagent_output_filter/src/system.rs` over in
  full: the noise-directory list, the search flag tables (short value-taking
  flags, long value-taking flags, format flags that disqualify a rewrite), the
  unsupported `find` predicate list, `ls` and `tree` rewrites, the search
  rewrite with its cluster parser for bundled short flags, `ls` output
  compaction with octal permissions and human sizes, tree output trimming, the
  `find` walker over the `ignore` crate with its own glob matcher, `find`
  output grouping with the extension histogram, search output parsing over the
  NUL-separated shape the rewrite asks for, the grouped-versus-plain decision
  and its 200-match and 25-per-file caps, search line truncation around the
  match, path compaction, the read modes (`cat`, `cat -n`, `head`, `head -N`,
  `tail -n N`) with their argument shapes, the filesystem read execution, smart
  truncation by signature and import lines, and `allows_empty_output` for the
  deliberate `tail -0`. Keep the iterative single-backtrack glob matcher and its
  timing test: the recursive form it replaced took minutes on a pattern a model
  can write
  (`../notagent-main-rust/crates/notagent_output_filter/src/system.rs:786`).
  Rename `parse_rtk_find_args` and reword the `find` rejection message so
  neither carries the upstream name. Rationale: this is the largest and most
  detail-dense file, it owns the two riskiest execution modes, and its
  rejection paths are what stop a rewritten command from silently changing what
  a search means.

- [x] 7. **Port the public API and the never-worse guard.** Bring
  `../notagent-main-rust/crates/notagent_output_filter/src/lib.rs` over in full:
  `PreparedInvocation` with its four accessors, `FilteredOutput`,
  `ExecutedOutput`, the `prepare` flow including the `declined` flag that lets a
  user-named output format outrank content detection, `native_filter_eligible`
  and `format_is_absent_or`, every rewrite arm, the compound-command rule that
  stands a system filter down behind a pipe, the Windows guard, `filter_inner`
  with its `catch_unwind` isolation, the fallback to content detection, and the
  four guards that send output back unchanged: an unchanged result, an empty
  result where raw output existed (unless `allows_empty_output`), a result that
  estimates larger than the raw, and a panic. Port `estimate_tokens`,
  `measure_retention` and `builtin_filter_count` too, renaming the
  measurement's upstream-named field. Rationale: the guards are what make the
  feature safe to switch on — every one of them is a promise that filtering
  never costs the agent information it would otherwise have had, and dropping
  any of them would be exactly the kind of simplification this plan forbids.

- [x] 8. **Bring the reference test suite across as the oracle.** Port all 50
  tests from the five reference files — 21 in `lib.rs`, 5 in `command.rs`, 6 in
  `native.rs`, 14 in `system.rs`, 4 in `toml_filter.rs` — with the same cases,
  the same expected values and the same names, as `CONVENTIONS.md` section 6
  requires for ported suites. Three of them are inventory pins that must keep
  their exact numbers: 63 filters with 154 fixtures and exactly one
  stderr-filtering filter, every inline fixture passing, and the aggregate
  fixture measurement (154 fixtures, 6206 raw tokens, 3238 reference tokens,
  3199 reduced, 105 reducing, 37 unchanged, 12 never-worse passthroughs).
  Rename only what the naming rule forces. Rationale: these tests are the only
  practical proof that a 5000-line port behaves like its source, and the pinned
  aggregates catch a filter that silently stopped matching in a way individual
  cases would not.

- [x] 9. **Decide and document the stream mapping, then wire filtering into the
  foreground bash path.** The v2 bash tool has one merged output stream where
  the reference has two, so the filter must be called with the merged text as
  stdout and an empty stderr; record that as a deviation in the module header
  with the reason. In `crates/notagent/src/core/tools/bash.rs:1495`, after
  the pipeline is finished and before `format_output`, run the prepared filter over
  the snapshot content and substitute the filtered text when the filter claims
  it. The exit-code and abort paths at `crates/notagent/src/core/tools/bash.rs:1477`
  keep their current wording; filtering changes what the text says, not what
  the status line says. Rationale: this is the point where every foreground
  command's output becomes the tool result, so it is the one place that has to
  see the filter, and doing it after `finish` means the filter always sees a
  complete output, which is what its buffered API requires.

- [x] 10. **Wire the execution rewrite and the override into command
  execution.** Prepare the invocation from the model's `command` argument
  before `command_prefix` is applied, because a prefixed command is multi-line
  and the classifier refuses those
  (`crates/notagent/src/core/tools/bash.rs:1390`); substitute the rewritten
  command into the prefixed form afterwards. Feed the prepared invocation's
  execution form to
  `resolve_spawn_context` while everything the user sees — the render call, the
  task description, the error text — keeps naming the original. Add the
  execution-override path for `find` and the read commands, running the
  filesystem adapter on a blocking task instead of calling
  `BashOperations::exec`, mirroring
  `../notagent-main-rust/crates/notagent_app/src/tool_executor.rs:797`. Stand
  the override down whenever the tool was built with operations other than the
  local ones: v2 lets a caller run commands on another machine
  (`crates/notagent/src/core/tools/bash.rs:143`), and a local filesystem
  adapter would then answer about the wrong filesystem — a failure mode the
  reference cannot have and therefore does not guard against. Honour
  `buffers_live_output` by suppressing live updates for system filters, and
  implement the `should_retry_original` re-run for searches whose rewritten
  output would not parse. Rationale: without the rewrite, half the native
  filters never get the machine-readable input they were written for, and
  without the override guard the feature would answer questions about the wrong
  machine.

- [x] 11. **Give background and managed tasks the same treatment.** Apply the
  rewrite to `ShellTaskSpec::command` at
  `crates/notagent/src/core/tools/bash.rs:1417`, and run override-backed
  commands verbatim there, exactly as the reference does at
  `../notagent-main-rust/crates/notagent_app/src/tool_executor.rs:178`: a
  background job has to stream and be killable, and a filesystem adapter
  provides neither. Filter the output of a managed foreground run in
  `run_managed` at `crates/notagent/src/core/tools/bash.rs:1586`, and decide
  and document whether `task_output`
  (`crates/notagent/src/core/tools/task_tools.rs:388`) filters its tail — the
  reference filters completed background jobs, and an unfiltered tail would
  reintroduce exactly the flood the feature removes. Rationale: the reference
  learned this the hard way — the same `go test ./...` was quiet in the
  foreground and spammed the context from the background, which is the gap its
  own comment records.

- [x] 12. **Add the setting, the slash command and the settings entry.** Add an
  `atomic_leases`-shaped `bash_filter` field to the settings struct at
  `crates/notagent/src/core/settings_manager.rs:222` with
  `get_bash_filter_enabled` defaulting to off and `set_bash_filter_enabled`
  writing the `bashFilter` key. Add `bash-filter` to `BUILTIN_SLASH_COMMANDS`
  (`crates/notagent/src/core/slash_commands.rs:62`) with an `[on|off]` argument
  hint and update the two order tests, add the handler and dispatch arm in
  `crates/notagent/src/modes/interactive/interactive_mode.rs` next to the
  `/leases` and `/index` handlers, and add a "Bash filter" row to the settings
  selector with its config field, callback and dispatch id, plus the fixture
  field in `crates/notagent/tests/settings_selector.rs`. Wire the gate into the
  bash tool through `BashToolOptions` as a call-time closure, in both the
  parent and the subagent branches of
  `crates/notagent/src/core/agent_session.rs:1494`, following exactly the
  pattern `lease_gate` established. Rationale: read at call time the switch
  applies to the next command rather than the next session, and wiring
  subagents too means a delegated `cargo test` is as quiet as a direct one.

- [x] 13. **Purge the upstream name and record provenance outside the code.**
  Grep the whole workspace case-insensitively for the upstream name and confirm
  zero hits in Rust sources, TOML filter files, comments, test names, setting
  names and user-visible strings. Rewrite every reference file header into a
  description of what the module does rather than where it came from. Add a
  top-level `NOTICE` file recording the upstream project, its Apache-2.0
  licence, the pinned commit the reference took its baseline from, and which
  parts derive from it — a data file, not code, so the naming rule holds while
  the licence obligation is met. Rationale: the rule is explicit and easy to
  violate by accident, since the upstream name appears in file headers, a
  function name, a struct field, several doc comments and one message the model
  actually reads.

- [x] 14. **Add the v2-side integration tests and run the gate.** Cover the
  foreground filter path, the rewrite substitution under a command prefix, the
  override path and its stand-down under custom operations, the live-buffering
  suppression, the search retry, the background rewrite, the filter being off by
  default and on after `/bash-filter on`, and an end-to-end `/bash-filter`
  scenario in `crates/notagent/tests/interactive_e2e/` alongside the `/leases`
  one. Port the nine filter-related tests from
  `../notagent-main-rust/crates/notagent_app/src/tool_executor.rs:1510` that
  still have a counterpart here. Bump the workspace version to 0.1.20 and run
  `./scripts/check.sh` until format, clippy with warnings denied, and the whole
  test suite are green. Rationale: the unit tests prove the filter matches its
  source, and only these prove the bash tool actually uses it — which is the
  half a port of this shape most easily gets wrong.

## Verification Criteria

- `./scripts/check.sh` passes: `cargo fmt --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, and `cargo test --workspace`.
- All 50 ported reference tests pass with unchanged expected values, renamed
  only where the naming rule forces it.
- The inventory pins hold: 63 built-in filters, 154 inline fixtures, exactly one
  filter with `filter_stderr`, and every inline fixture reproducing its expected
  output.
- The aggregate fixture measurement reproduces the reference numbers exactly:
  6206 raw tokens, 3238 reference tokens, 3199 reduced, 105 reducing, 37
  unchanged, 12 never-worse passthroughs.
- `rg -i rtk` over `crates/`, `plans/` excepted, returns no hits in any `.rs` or
  `.toml` file, and no user-visible string contains the upstream name.
- With the filter off, `bash` output is byte-identical to today's for every
  existing bash test — the whole suite in
  `crates/notagent/tests/bash_tool.rs` and `bash_background.rs` passes
  unchanged.
- With the filter on, `cargo test` output compacts to `cargo test: N passed (M
  suites)`, `go test ./...` runs as `go test -json ./...` while the transcript
  still shows `go test ./...`, and `ls` returns the compacted listing.
- A command whose filtered output would estimate larger than its raw output
  comes back raw, and a filter that panics leaves the raw output intact.
- `find` and `cat`/`head`/`tail` answer without spawning a child process when
  the tool uses local operations, and spawn normally when it does not.
- `/bash-filter` with no argument toggles, `on` and `off` set, an unusable
  argument is reported and changes nothing, and the choice survives in
  `settings.json` under `bashFilter`.
- A subagent's bash calls are filtered on the same terms as its parent's.

## Potential Risks and Mitigations

1. **The merged output stream changes what the filters see.**
   The reference hands each filter a separate stdout and stderr; v2 has one
   interleaved buffer, so a JSON post-processor (vitest, eslint, pip, ruff,
   golangci) can be handed a document with progress noise mixed into it and will
   refuse to parse.
   Mitigation: the refusal is already the designed behaviour — every JSON parser
   returns `None` on unparseable input and the never-worse guard sends the raw
   output through untouched, which is exactly what happens today with the filter
   off. Port the reference's `json_parsers_reject_unstructured_output` test and
   add a case that feeds a JSON filter a stdout/stderr mixture, so the
   degradation is pinned as passthrough rather than as damage.

2. **A rewritten invocation changes what a command means.**
   Appending `-json` or `--reporter=json` to a stage of a pipeline changes what
   the later stages read, and rewriting a search changes what its output looks
   like to whatever consumes it.
   Mitigation: port the classifier's `compound` flag and the rule that no
   rewrite is ever applied to a compound command, unchanged
   (`../notagent-main-rust/crates/notagent_output_filter/src/lib.rs:118`), and
   port `should_retry_original` so an unparseable rewritten search is re-run
   verbatim. Both have tests in the reference suite; they are not optional.

3. **The execution override answers about the wrong filesystem.**
   v2 allows remote `BashOperations`; a local filesystem adapter for `find` or
   `cat` would then answer about the local machine while the user is working on
   another one — a silent wrong answer, the worst failure mode this feature can
   have.
   Mitigation: gate the override on the tool having been built with the local
   operations, and cover it with a test that installs custom operations and
   asserts the command is executed rather than overridden. This guard is an
   addition over the reference, which has no such seam.

4. **The port drifts from the reference in ways the tests do not catch.**
   5000 lines of dense string formatting and caps offer many places for a
   transcription slip that no ported test happens to exercise.
   Mitigation: the three inventory pins and the aggregate token measurement are
   deliberately broad — they fail on any filter that stops matching or any
   pipeline stage whose output length changes. Beyond that, port the files whole
   rather than function by function, and diff the ported file against the
   reference before declaring a task done.

5. **A filter panics on real-world output.**
   Regex-heavy code over untrusted process output can panic on a shape no
   fixture contains, and a panic in a tool call is a lost turn.
   Mitigation: port the `catch_unwind` isolation of `filter_inner` and its
   fallback to raw output unchanged
   (`../notagent-main-rust/crates/notagent_output_filter/src/lib.rs:378`), and
   keep the iterative glob matcher whose recursive predecessor hung for minutes
   on a model-written pattern.

6. **Live streaming and buffered filtering disagree.**
   The user watches raw output stream past and then sees a compacted result,
   or a system filter's live output contradicts the adapter-shaped result.
   Mitigation: port `buffers_live_output` and suppress live updates exactly
   where the reference does — for system filters, whose output shape the adapter
   dictated. For the rest the reference streams raw and compacts at the end, and
   that is the intended experience, not a defect.

7. **Attribution is removed from an Apache-2.0 derivative.**
   Section 4 of the licence asks that attribution notices be retained.
   Mitigation: keep the provenance in a top-level `NOTICE` file, which is
   neither code nor comment, so the naming instruction and the licence
   obligation are both satisfied. Flag the trade-off to the user rather than
   deciding it silently.

## Alternative Approaches

1. **A separate workspace crate instead of a module.** Mirrors the reference's
   layout and keeps the filter compilable and testable on its own. Rejected
   because `CONVENTIONS.md` section 1 fixes one crate per TS package and this
   feature has no TS counterpart; the two existing port additions,
   `find_codebase` and `file_lease`, both live as modules of `crates/notagent`.

2. **A build script, as the reference uses.** Would keep the TOML embedding
   byte-identical to the reference including its build-time assertions.
   Rejected because this repository has no build script anywhere and its
   established embedding pattern is an explicit `include_str!` list
   (`crates/notagent/src/core/modes.rs:42`); the assertions lose nothing by
   becoming unit tests, and a missing file then fails a test rather than a
   compile.

3. **Splitting stdout and stderr in `BashOperations`.** Would let the filter see
   the two streams separately, as the reference does, and would let the
   `stdout_only` set and `filter_stderr` behave exactly as written. Rejected as
   out of scope: it changes a trait the TS original defines as a single stream,
   touches every implementation including the remote ones, and the never-worse
   guard already turns the resulting parse failures into plain passthrough.
   Worth revisiting if the JSON filters turn out to miss often in practice.

4. **Porting upstream's full `src/cmds` tree as well.** Would extend native
   coverage from 35 filters to the upstream superset of 47 command modules.
   Rejected here because the reference does not carry it, the tree is ~44k LOC
   with its own CLI and runner assumptions, and nothing in it has the reference
   test suite that makes a port of this size verifiable. A separate decision, not
   a silent extension of this one.

5. **Filtering inside the output accumulator rather than after it.** Would
   compact output as it streams and keep the temp file small. Rejected because
   every native filter is a buffered whole-output function — a cargo summary
   line cannot be produced from a prefix — and the reference's own design
   buffers for exactly this reason.
