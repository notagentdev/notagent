---
name: debug
description: Systematic debugging discipline. Use when something is broken, failing, throwing, or slow, before proposing any fix. Builds a reproducible failure signal first, then tests ranked hypotheses one variable at a time.
---

# Debug

No fix before the cause is understood, and no cause before the failure is reproducible. Symptom patches are the failure mode this discipline exists to prevent.

## First: a signal that goes red

Build one command that fails on this bug and will pass once it is fixed: a failing test at whatever seam reaches the bug, a scripted invocation diffed against known-good output, a replayed captured input, a differential run of two versions or two configurations. Spend disproportionate effort here — with a tight signal the rest is mechanical, without one no amount of reading code will save you.

Make the signal fast, deterministic, and specific to the reported symptom, not merely to "something failed". For a flaky failure, raise the reproduction rate until it is debuggable rather than hunting a perfect repro.

If you genuinely cannot build one, stop and say so, listing what you tried and what access or captured artifact would unblock it. Do not proceed to theories without a signal.

## Reproduce and minimise

Run the signal and watch it fail with the symptom the user described — a nearby failure is the wrong bug. Then shrink the reproduction one element at a time, re-running after each cut, until everything that remains is load-bearing. A minimal reproduction shrinks the space of possible causes and becomes the regression test later.

Read the complete error output, and check what changed recently: history, dependencies, configuration, environment.

## Hypotheses before probes

Write down three to five candidate causes, ranked, before testing any. Each must be falsifiable: state the prediction it makes — if this is the cause, then this specific change makes the failure disappear or worsen. A hypothesis without a prediction is a hunch; sharpen it or discard it.

Test one variable at a time. Tag every piece of debugging instrumentation with one unique marker so a single search removes all of it. When a hypothesis dies, form the next from what its death revealed — do not stack a second speculative fix on a first.

After three failed fixes, stop treating it as a bug in one line: the design around it is now the question, and that is worth raising before a fourth attempt.

## Fix and prove

Where a correct seam for a regression test exists, write the test first, watch it fail, apply the fix, watch it pass. Where no seam can exercise the real failure pattern, that absence is itself a finding — record it instead of writing a test that proves nothing.

Before declaring done: the original signal passes, every tagged probe is removed, nothing unrelated broke, and the confirmed cause is stated so the next reader learns what actually happened. Redact secrets from anything you show along the way.
