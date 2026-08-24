---
name: matchcore-payload-validation-belongs-to-condition
description: Use when reviewing MatchCore validation in PluginConditions, RequestRestriction, RequestAccessPolicy, or a new Condition consumer; payload invariants belong to Condition, not consumer copies.
status: fixed
finding_id: cl-02
closed: 2026-08-24
---

# MatchCore Payload Validation Belongs to Condition

## Conclusion

`keyMatch`, `jwtClaimMatch`, and `mtlsMatch` require at least one non-empty
`values` or `regex` list. The invariant is owned by `Condition::validate_at`,
which serves both entry-level conditions and `allOf` consumers. A plugin must
retain only its container rules and must not duplicate a subset of the matcher
payload validation.

A present `values` list is independently required to be non-empty, even when
`regex` supplies a usable matcher. Review these as two separate invariants;
testing only a completely default `MatchCore` misses the mixed-payload edge.

## Why

An empty `MatchCore` never matches. With `negate: true`, evaluating such an
invalid configuration would produce a tautology. Consumer-local checks also
drifted: RequestRestriction covered all three matcher variants,
RequestAccessPolicy covered only `keyMatch`, and plugin `run`/`skip`, body, and
dye conditions covered none.

The shared validation boundary keeps Controller `check_fields` and Gateway
compile acceptance aligned for every current and future `Condition` consumer.

## Review Rule

When adding a `Condition` payload invariant, implement it in the shared
validation path and test both entry-level and `allOf` callers. Consumer
validators may enforce grouping, mode, cardinality, and other container-owned
rules only.

## Reference

- `cl-02` (2026-08-24)
- `tasks/condition-layer/02-condition-unification/01-design.md` section 12
- `skills/04-review/plugin-metadata/unified-condition-layer.md`
