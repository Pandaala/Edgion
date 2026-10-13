---
name: semantic-transformation-reset-output
description: Preserve full WAF action validation while emitting only effective transformation indices after the final reset.
knowledge_type: regression-guards
---

# Semantic folds commit only surviving transformation indices

ARCH-227 removes discard-only index materialization from the cold semantic
frontend. `inspect_fold` validates the complete original action range and records
the final reset boundary and effective count. After successful inspection,
`fold_local_rules` emits transformation indices only after that last `t:none`.
Without a reset it emits all transformations in their original order.

The first pass still validates every fold before aggregate effective/owned/retained
admission and fallibly reserves the final effective count. The commit pass repeats
full inspection; only its output writes move to the surviving suffix. Invalid or
forbidden actions before a reset remain errors, as do duplicate IDs and status/pass
conflicts. Logical charges, final limits, reset identity and chain adjacency remain
unchanged. No discarded prefix can cause ordinary Vec growth beyond the admitted
final index count.

Tests verify zero index capacity for a fresh final-empty owner, a surviving suffix
between neighboring folds, and invalid pre-reset actions. Existing exhaustive suffix,
error-precedence and exact logical-boundary tests remain applicable. Nonzero vector
capacity is allocator-dependent; logical semantic charges are not measured heap use.
This is bounded cold-frontend cleanup, not a request-path or RSS benchmark claim.

Re-evaluate when reset semantics, classified action storage, fold validation order,
final-count reservation or compiler consumption changes. Full-original validation
must precede any profile/rule selection; source records are not pruned by this fix.
