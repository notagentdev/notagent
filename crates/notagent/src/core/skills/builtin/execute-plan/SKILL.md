---
name: execute-plan
description: Execute a recorded plan file task by task with status tracking in the plan itself. Use when the user names a plan file or asks for a plan to be executed. Needs full access; the plan file is updated in place as tasks complete.
---

# Execute a Plan

Work through a plan file until every task is resolved, keeping the file itself as the progress record.

## Before starting

Read the whole plan, not just the task list. Review it critically: if a task is unclear, contradicts the code as it exists now, or has a gap that prevents starting, raise that with the user before doing anything. A plan written earlier can be behind the repository.

Mirror the plan's tasks in your todo list so progress is visible outside the file.

## Status markers

Tasks use one checkbox state each, updated in the plan file as you go:

- `[ ]` pending
- `[~]` in progress
- `[x]` done
- `[!]` failed, with a one-line reason added to the task

## The loop

Take the first pending task. Mark it `[~]` in the plan file. Do the work. Verify it against the plan's verification criteria — the criteria state what must be observably true; how to check them comes from the project itself. Mark the task `[x]`, or `[!]` with the reason, and move to the next pending task.

Stop and report instead of guessing when an instruction is unclear, a dependency is missing, or verification keeps failing after honest attempts. A wrong guess executed at speed costs more than the question.

## Finishing

When no task is pending, re-read the plan file and confirm nothing is left `[ ]` or `[~]`. Then report: what was done, what was verified and how, and anything marked `[!]` with its reason. An unresolved task is stated plainly, never silently dropped.

Updating the plan file requires write access; this skill does not work in a read-only mode.
