---
name: create-plan
description: Produce an implementation plan before code is changed. Use when the user asks for a plan, roadmap, or strategy, or when a task needs structured analysis and breakdown before implementation. Research-only; records the plan with plan_create and changes nothing else.
---

# Create a Plan

A plan is a decision the reader can act on: what should change, why, in which files, and how the result will be recognised as correct. It is not a diff written in advance.

## Research first

Establish how the code actually behaves by reading it, not by inferring from names. Follow a flow end to end before you draw a conclusion from it. Cite every claim as `path:line` so the reader can check you.

When two readings of the request lead to materially different work, say so and name the assumption you proceed under — or ask, if proceeding under the wrong one would waste the work.

## Structure

Write the plan with these sections:

- **Objective**: the goal and the expected outcome, stated so someone who was not in the conversation understands what is being built and what is explicitly out of scope.
- **Settled Decisions and Assumptions**: choices already made and the assumptions the plan rests on, each with its reason.
- **Implementation Plan**: checkbox tasks, in execution order. Every task names what changes, why, which files are affected, and how the change integrates with what is already there. When tasks will be executed by separate agents that cannot see each other, state on each task the exact names and signatures it consumes from earlier tasks and produces for later ones.
- **Verification Criteria**: observable outcomes, one per line. State what must be true afterwards, not which command to run — the executor knows the project's own verification entry points, and a criterion outlives a command line.
- **Risks and Mitigations**: what can go wrong and what the plan does about it.
- **Rejected Alternatives**: approaches considered and the reason each lost.

## Rules

- No code, no code snippets, no code examples. Describe the change in prose. Exact names appear only where the exact form is the decision — a public signature, a file name, a configuration key — written as text.
- No placeholders. "Handle edge cases", "add appropriate validation", "similar to task 3" are plan failures: every task must carry the actual content the executor needs.
- Do not promise work you have not scoped. If part of the request is blocked or ill-defined, say which part and why.

## Self-review

Before recording the plan, check it against the request with fresh eyes: every requirement maps to a task, no task references something no task defines, and names used in later tasks match the names earlier tasks introduce. Fix what you find and move on.

## Record it

Record the plan with `plan_create` rather than only in your reply. A plan that exists only in the conversation is gone when the session ends, which is precisely when someone wants to come back to it.
