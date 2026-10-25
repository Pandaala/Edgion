---
name: weighted-round-robin-normalize-weights
description: Review BackendRef WRR normalization, conservative sequence bounds, unit-weight sequence construction and historical unclamped-weight findings.
status: fixed
finding_id: lb-F005
closed: 2026-05-24
knowledge_type: regression-guards
---

# WeightedRoundRobin normalization and sequence construction

## Current contract

`WeightedRoundRobin::new` in `lb/backend_selector/weighted_selector.rs` validates
nonempty items, matching lengths, at most 65,535 items and a nonzero total before
normalizing weights. `normalize_weights` first divides by the positive weights'
GCD, preserving ratios exactly. If the reduced sum exceeds 10,000, proportional
integer scaling with a minimum of one retains every positive share but only
approximates its original ratio. Zero weights remain zero.

The scaling target is not a strict sequence-length cap: the minimum-one rule
can make the sum exceed 10,000. A conservative bound is 10,000 plus the positive
item count, itself bounded by the constructor's item-count check. Thus the former
universal approximately 20 KB sequence claim was too strong. The sequence stores
u16 indices; total heap usage also includes items, weights and construction
scratch. Normalization removes dependence on the unbounded original weight sum,
not dependence on item count.

After normalization, all-unit weights emit exactly `0..n` directly. At step k,
unselected items have current weight k+1 and previously selected items have
k+1-n; first-index tie breaking picks the next unselected index. This preserves
the full sequence while reducing this case's construction from O(n squared) to
O(n) and omitting both current/effective-weight scratch vectors. Mixed zeros and
nonuniform normalized weights retain the smooth algorithm and its
O(item count times normalized weight sum) construction work. Each round updates
scores and tracks the maximum in one traversal, preserving strict first-index
tie breaking; subtraction still occurs after every score update. This removes
the separate maximum-search traversal without changing scratch storage.

The published sequence, atomic cursor, wrapping behavior, per-pick remainder,
item ownership and error mapping are unchanged. Construction is cold in the
compiled BackendRef generation, not lazy on the first request. Current production
construction is through `BackendSelector::create_state`; the historical webhook
WRR caller no longer exists. `with_equal_weights` shares the same constructor.
See [generation ownership](backend-ref-selector-generation-ownership.md) and
[fallible construction](wrr-new-result-api.md).

## Historical correction and scope

lb-F005 (2026-05-24) fixed expansion of original weights directly into a sequence.
Two weights near i32::MAX could previously request billions of entries. GCD
reduction and proportional scaling address that original-input expansion; they
do not prove exact ratios after rounding or a universal 10,000-entry bound.
The original choice retained immutable precomputation and lock-free selection
rather than introducing a mutable scheduling heap. This remains distinct from
RoundRobinSelector/ConsistentHashRing's separate per-backend weight handling.

Revalidate the constructor and normalization before reporting the historical
unclamped-weight defect. New count, arithmetic, normalization or failure-policy
defects are not suppressed by that historical correction.

## Verification and re-evaluation

Mounted tests under `edgion-gateway/tests/unit/lb/backend_selector/` cover GCD,
zero preservation, large/coprime inputs, fallible construction and selection.
The exact-order regression compares unit-weight cycles at several counts and
original weight magnitudes, plus literal nonuniform and mixed-zero sequences;
it passed against the original generator before the shortcut was added. A
post-change 65,535-item test checks the cast boundary and complete cycle above
the scaling target without running the old quadratic generator at that size.

These establish output and boundary behavior, not measured throughput or total
heap savings. Re-evaluate normalization, tie ordering, count limits and shared
cursor ownership if the algorithm or constructor changes.
