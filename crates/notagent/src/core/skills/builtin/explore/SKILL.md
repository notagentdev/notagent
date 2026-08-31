---
name: explore
description: Read-only investigation of a codebase. Use for mapping how something works, tracing a data flow, or answering a question about the code without changing it. Produces findings with citations, not modifications.
---

# Explore

The deliverable is an accurate account of how the code behaves, precise enough that the reader can act on it without re-doing the reading.

## Method

- Establish behaviour by reading the code, not by inferring from names. A function called `validate` proves nothing about what is validated until you have read it.
- Follow a flow from its entry point to its final use before describing it. The middle of a flow is where assumptions die.
- Prefer primary sources: the code itself, its tests, its own documentation — in that order. Tests state what the authors promised; prose states what they intended; only the code states what happens.
- Read enough context around a match. A grep hit is a lead, not a finding.

## Reporting

- Cite every claim as `path:line` so the reader can verify it.
- Separate observed fact from inference, and say which is which. "The retry loop caps at three attempts" is a reading; "so a fourth failure reaches the caller" is a deduction — mark the difference.
- Say what you did not examine. An account that is silent about its edges reads as complete when it is not.
- Keep a running note of what you read and what each file contributed, so the final answer can be checked against its sources.

## Boundary

When the answer turns out to require a change, describe the change — where, what, and why — rather than making it. Investigation that quietly turns into modification has left its mandate.
