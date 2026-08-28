# Contributing

## The one rule

**Understand what you are handing over.**

You have to be able to say what a change does and how it meets the rest of the
system. If you cannot, it is not ready — however well it reads.

Letting an agent write the code is normal here; most of this repository was
written that way. What does not pass is handing on what an agent produced
without having understood it. Generated code is confident by construction: it
names APIs that look plausible, follows conventions that look like ours, and
reports success it never checked. Reading it is the part that cannot be
delegated.

The same rule in its practical form: do not guess at anything outside this
repository. Read the documentation of the API you are calling, or probe the
endpoint and look at what comes back. A field name that sounds right but does
not exist costs more than an open question, because it fails quietly and the
next person inherits it.

## Before you hand something over

```sh
scripts/check.sh
```

Format, clippy with warnings as errors, and the whole workspace's tests. It has
to be green. A test that was already failing before your change is reported —
verify it on a clean tree with `git stash` — not fixed in passing and not
quietly left out.

If a test fails because of your change, decide deliberately whether the code or
the expectation is wrong, and say which you concluded. A pinned expectation that
no longer matches is sometimes the bug and sometimes exactly the point.

## Scope

Agree on what a change is before writing it. Anything that touches the system
prompt, agent behaviour, providers, tool schemas, permissions, the TUI, or the
runtime architecture is settled first, not shown as a finished diff — those are
the places where two reasonable readings of the same request lead to entirely
different work.

Then stay inside what was agreed. If the fix turns out to need a neighbouring
change, say so and get that agreed too, rather than widening the diff on your
own judgement. A change that arrives larger than the thing that was asked for
has to be reviewed from scratch, and the part nobody asked for is where the
surprises live.

Large refactors need their own agreement. So does a second code path for an old
format: never add one without asking first.

## The rules that apply while you work

[AGENTS.md](AGENTS.md) is the working guide: crate layout, build and test
commands, test and comment conventions, commits, versioning. Run your agent from
the repository root so it picks that file up, and hold it to what is in there.

[CONVENTIONS.md](CONVENTIONS.md) is the binding document underneath it and goes
deeper on module layout, error handling, async, and dependencies. It is written
in German; everything else — code, comments, commits — is English.

## Licensing

This project is MIT. By contributing you agree that your contribution is
licensed under those terms; there is no separate CLA.

Substantial portions derive from pi, which is MIT as well, and one subsystem
from an Apache-2.0 work. Both are recorded in [NOTICE](NOTICE). If you carry
code in from somewhere else, it goes in there too, with its licence and a
sentence on what was changed — before the code lands, not afterwards.

## Reporting a problem

Keep it to one screen, and make it something somebody can act on without asking
you a question first:

- The version (`notagent --version`), the operating system, and the terminal.
- The provider and model. Behaviour differs enough between them that a report
  without this often cannot be placed at all.
- The exact steps, as commands and input rather than as a description of them.
- What you expected, and what happened instead — both, separately.
- The relevant output. Not the whole transcript: the part that shows it, with a
  sentence saying what to look at.
- The crate it appears to be in, if you can tell — `notagent`, `notagent-ai`,
  `notagent-tui`, and so on.

What does not help: "doesn't work" on its own, a long transcript pasted without
a summary, and anything that leaves the reproduction to be guessed at.
