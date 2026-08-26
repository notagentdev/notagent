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

## The rules that apply while you work

[AGENTS.md](AGENTS.md) is the working guide: crate layout, build and test
commands, test and comment conventions, commits, versioning. Run your agent from
the repository root so it picks that file up, and hold it to what is in there.

[CONVENTIONS.md](CONVENTIONS.md) is the binding document underneath it and goes
deeper on module layout, error handling, async, and dependencies. It is written
in German; everything else — code, comments, commits — is English.

## Reporting a problem

Say what you did, what happened, and what you expected instead. A command and
its output beats a description of the output. If it only happens with a
particular provider, model, or terminal, name it — those three account for most
of what cannot be reproduced.
