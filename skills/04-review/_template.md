---
name: <semantic-slug>
description: Use when reviewing <owning function and concrete behavior>.
knowledge_type: <one directory name from knowledge-routing.md>
---

# <Rule or decision title>

## Conclusion and scope

<State the current invariant, accepted choice, external constraint, correction, or
counter-evidence. Name the owning component/function and relevant resource/fields.
Distinguish current behavior from historical context.>

## Rationale and evidence

<Link the canonical authority and cite the decisive current source/call chain.
Explain why the conclusion follows. Preserve useful finding IDs and original dates;
do not claim that migrating a record reverified its code or tests.>

## Alternatives and limits

<For a tradeoff: benefit, accepted cost/risk, and rejected alternatives.
For a compatibility constraint: exact dependency/protocol/peer boundary and mitigation.
For a correction: trigger, root cause, fixed behavior, and regression evidence.
For a false positive: the incorrect premise and scope of counter-evidence.
For superseded guidance: inactive notice and successor. Delete inapplicable prompts.>

## Verification and re-evaluation

<Record actual commands/tests/results when known, or explicitly say historical evidence
only. State changes that would require reconsideration. Link unresolved tasks if any.>

## Resolution references

- <Stable finding ID, original disposition/date, and useful source reference.>

<!-- Update an existing semantic rule before creating a new file. Store at
04-review/<function>/<knowledge-type>/<semantic-slug>.md. Keep this template and
knowledge-routing.md as authoring references; do not create a finding archive. -->
