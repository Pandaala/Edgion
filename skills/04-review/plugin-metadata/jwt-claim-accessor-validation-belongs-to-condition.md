---
name: jwt-claim-accessor-validation-belongs-to-condition
description: Use when reviewing jwtClaimMatch validation; its claim accessor must be non-empty in the shared Condition path before runtime extraction or negate.
status: fixed
finding_id: cl-03
closed: 2026-08-24
---

# JWT Claim Accessor Validation Belongs to Condition

## Conclusion

`jwtClaimMatch.claim` is required and must not be empty or whitespace-only.
The invariant belongs to `Condition::validate_at`, alongside the shared
MatchCore rules, so plugin conditions, RequestRestriction, RequestAccessPolicy,
and future consumers accept the same configuration shapes.

## Why

An empty claim path extracts no values. Runtime matching therefore produces an
ordinary non-match, which `negate: true` can invert into a tautology. Rejecting
the malformed accessor at configuration time avoids turning a typo or an empty
template result into a permissive JWT rule.

Do not replace this rule with consumer-local checks or runtime special cases.
Dot-path syntax validation beyond non-empty remains a separate product choice.

## Reference

- `cl-03` (2026-08-24)
- `tasks/condition-layer/02-condition-unification/01-design.md` section 12
- [matchcore-payload-validation-belongs-to-condition.md](matchcore-payload-validation-belongs-to-condition.md)
