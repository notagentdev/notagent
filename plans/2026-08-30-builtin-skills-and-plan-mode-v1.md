# Built-in Skills and a Skill-Aware Plan Mode

## Objective

Ship four skills with the binary — `create-plan`, `execute-plan`, `explore`, `debug` — and make plan mode ask for the planning skill instead of carrying the planning method itself.

The mechanism is deliberately small: the binary embeds the four files and writes any that are missing into the user skills directory on startup. From there they are ordinary skills — the loader, the `skill` tool, the `task` tool, `/skill:name`, the system prompt listing, and the `read` tool all work on them unchanged, and a user edits or deletes them like any skill of their own.

## Settled Decisions and Assumptions

- Delivery is materialise-if-absent: each embedded skill is written to `<agent_dir>/skills/<name>/SKILL.md` only when that file does not exist. The agent directory is user-writable by definition, no loader or tool changes are needed, and a user's edit survives because an existing file is never touched. The cost is that an upgrade does not update an already-written skill; deleting the file restores the current shipped version on next start.
- The existing precedence applies unchanged: skills load first-wins with the user directory before the project directory (`crates/notagent/src/core/skills.rs:530-536`), so these skills behave exactly like user-authored ones.
- Plan mode instructs, it does not inject. A skill forced into every switch would sit beside whatever planning skill the user loaded, and where the two disagree the model holds two authoritative texts with no rule for which wins. So `10-plan.md` tells the model to load `create-plan` — unless the user asked for a different approach or a planning skill is already loaded. The `skill` tool already states the matching rule for itself (`crates/notagent/src/core/tools/skill.rs:44`).
- The skills are language- and project-neutral: no build tool, test runner, language, or repository convention appears in them. Project knowledge comes from context files (`crates/notagent/src/core/system_prompt.rs:49-62`). No code blocks, no emoji, English prose.
- Plans contain no code: they are written in a shell without `bash`, and the executor reads the live code. File naming stays with `plan_create` (`crates/notagent/src/core/tools/plan_create.rs:36`).
- The read-only gate is untouched; a skill changes what the model knows, never what it can call (`crates/notagent/src/core/modes/shells.rs:89-118`).

## Implementation Plan

- [x] 1. Embed and materialise the skills. Add a constant table in `crates/notagent/src/core/skills.rs` pairing each of the four names with its `include_str!` content from new files under `crates/notagent/src/core/skills/builtin/<name>/SKILL.md`, plus one function that writes each entry to `<agent_dir>/skills/<name>/SKILL.md` when that file is absent, creating directories as needed and ignoring write failures with a diagnostic. Call it in the resource loader before the first skill load (`crates/notagent/src/core/resource_loader.rs:488-500`), so the files exist by the time `load_skills` walks the directory.

- [x] 2. Rewrite `crates/notagent/src/core/modes/builtin/plan/10-plan.md`. Keep `shell: read-only`; the body states what the mode is — read-only, enforced by the runtime, `plan_create` as the one way to write — and ends with the instruction to load `create-plan` with the `skill` tool, unless the user asked for a different approach or a planning skill is already loaded, in which case follow that one and do not load `create-plan` beside it. The method paragraphs (`10-plan.md:9-19`) move into `create-plan`.

- [x] 3. Write `create-plan`: when to load it (plan, roadmap, strategy, structured breakdown); read before concluding and cite `path:line`; state assumptions; plan structure of objective, settled decisions, checkbox tasks with what, why, where, and how it integrates, verification criteria as observable outcomes rather than commands, risks with mitigations, rejected alternatives; no code, no placeholders; a self-review that maps every requirement to a task; record with `plan_create`.

- [x] 4. Write `execute-plan`: load when the user names a plan file; read and review the whole plan first, raising concerns before starting; mirror tasks in `todo_write`; per task mark `[~]` in the plan file, do the work, verify, mark `[x]` or `[!]` with a reason; stop and report instead of guessing on unclear instructions or repeated verification failure; re-read the plan at the end; needs a worker shell to edit the file.

- [x] 5. Write `explore`: load for read-only investigation; establish behaviour by reading, not by inferring from names; follow flows end to end; cite `path:line`; separate observed fact from inference; report what was not examined; describe needed changes instead of making them. Write `debug`: load before proposing any fix; build a pass/fail signal that goes red on this bug before theorising; reproduce and minimise; three to five ranked falsifiable hypotheses before testing any; one variable per probe, instrumentation tagged for one-search removal; regression test before the fix where a seam exists; after three failed fixes question the design; on completion re-run the signal, remove probes, state the confirmed cause; redact secrets.

- [x] 6. Tests and docs. In `crates/notagent/tests/skills.rs`: materialisation writes a missing skill and leaves an existing file untouched; the four load through `load_skills` with valid names and descriptions. In `crates/notagent/tests/modes.rs`: the shipped plan mode's injection names `create-plan` and the `skill` tool, and still exposes no mutating tool (`tests/modes.rs:165-171`). In `README.md`: one short section naming the four skills, that they are written once into the user skills directory and are the user's to edit, shadow, or delete, and that plan mode asks for `create-plan` unless another planning skill is in use.

## Verification Criteria

- First start with an empty `<agent_dir>/skills` creates the four skill directories; a second start changes nothing; an edited SKILL.md keeps its edit; a deleted one reappears.
- The four skills show up in the system prompt listing, the `skill` tool, and the `task` catalogue with no loader change.
- Switching to plan mode delivers only the `<mode>` block, whose text names `create-plan`, the `skill` tool, and both exceptions.
- Plan mode still exposes no mutating tool; `plan_create` and `skill` are present.
- No skill body contains a code fence, an emoji, or a language, build-tool, or test-runner name.
- The workspace check script is green.

## Potential Risks and Mitigations

1. **An upgrade does not refresh an already-written skill.** Accepted for simplicity; documented, and deleting the file restores the shipped version.
2. **The model plans without loading the skill, or loads it beside a user's planning skill.** The instruction names the tool and both exceptions, and the `skill` tool's description already forbids loading duplicated guidance; checked by reading transcripts, not by a unit test.
3. **These skills sit in the user directory, so they take precedence over project skills of the same name.** That is the existing rule for every user skill; a project that wants its own `debug` needs the user file edited or removed, which the README states.

## Alternative Approaches

1. **Embed with virtual paths and teach every consumer to read embedded bodies.** Rejected: it touches the `Skill` struct, three body readers, and the prompt listing for a result the file-based route gets for free.
2. **Inject the skill on every switch via a mode frontmatter field.** Rejected: forced guidance conflicts with a user's own planning skill and pays the skill's length on every switch.
3. **Ship the files via the cask.** Rejected: `cargo install` and local builds would not have them.
