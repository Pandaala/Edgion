---
name: condition-set-single-owner
description: Use when reviewing condition containers and predicate ownership; separates current ConditionSet semantics from retired flat grouping.
knowledge_type: mandatory-design
---

# ConditionSet and predicate ownership

## Conclusion and scope

Use the shared `Condition` predicate enum and bounded `ConditionSet` container.
Do not add consumer-local predicate enums or revive the old flat grouping fields.
`KeyGet` remains local and I/O-free. The common condition compiler and evaluator
own predicate validation and combination semantics across plugin consumers.

## Authority and evidence

The current contract is documented in [plugin architecture](../../../01-architecture/02-gateway/05-plugin-system.md)
and the [graded unification rule](../../../00-design-rules.md).
The [original condition-layer decision](../superseded/unified-condition-layer.md)
retains its useful single-owner rationale, but its old container examples are historical.
Follow current schema and consumer documentation when changing grouping fields.

Controller credential resolution uses `refs::condition_fields::PreparedConditionReferences`
after ConfigData expansion. Its synchronous visitor covers entry skip/run,
request/response body gates, both dye directions, RequestRestriction and effective
RequestAccessPolicy IP rules and expanded URL candidates. Inactive and unsupported
stages remain visitable to clear resolved credentials, but mint no reference token.
Each leaf loan retains the actual EdgionPlugins source, Boolean position and
reference wire; no API accepts an unrelated leaf plus caller-supplied owner.

Effective RAP profiles and candidates belong to this synchronous catalogue.
Do not include them in the raw-owner frozen topology used by asynchronous
credential publication: expansion legitimately changes them without changing
the raw resource. Condition tokens deliberately reject freezing. Profile-map
insertion order is immaterial; profile keys, rule branches, group/rule names,
candidate positions and reference ordering remain part of the binding.

## Verification and re-evaluation

This page separates existing current architecture from historical review examples;
it does not introduce a new runtime contract or claim new tests. Re-evaluate only
with an explicitly changed shared architecture, updating every affected consumer.

## Resolution references

- `condition-layer-s15`: original predicate unification decision.
- `CSD-1`: successor bounded ConditionSet grouping design.
