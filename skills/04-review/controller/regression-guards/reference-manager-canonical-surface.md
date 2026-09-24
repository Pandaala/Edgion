---
name: reference-manager-canonical-surface
description: Use when reviewing Controller reference-manager cleanup APIs and exports.
knowledge_type: regression-guards
---

# Reference-manager APIs use their canonical names

## Conclusion and scope

Controller reference managers clear an owner's dependencies through
`clear_value_refs`. `ResourceRef` and `RefManagerStats` come from
`resource_processor::ref_manager` or its top-level exports. The cross-namespace
manager uses the same `ResourceRef` type.

## Rationale and evidence

The forwarding `clear_resource_refs` method, unused `CrossNsResourceRef` alias,
and `secret_utils` re-exports added public vocabulary without distinct state
or behavior. Production callers were moved to `clear_value_refs`, and the
redundant exports were removed. This preserves the serial reference cleanup
operation and manager type.

## Verification and re-evaluation

The reference-manager tests still exercise clear-and-add semantics. Re-evaluate
if a genuinely different resource-reference type or cleanup operation appears.

## Resolution references

- `review-results/controller-gateway-overengineering-20260924-031121/ISSUE-004`.
- `review-results/controller-gateway-overengineering-20260924-031121/ISSUE-005`.
- `review-results/controller-gateway-overengineering-20260924-031121/ISSUE-006`.
