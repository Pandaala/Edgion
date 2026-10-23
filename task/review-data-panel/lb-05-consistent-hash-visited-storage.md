# Consistent-hash fallback visited storage

Status: deferred performance decision for the user. Baseline: `55011ad47`.
No runtime change, selected replacement or measured latency benefit.

## Evidence and decision

`edgion-gateway/src/lb/selection/consistent_hash.rs::SeenSet` uses a u128 mask
for at most 128 backend slots. Larger lists keep the first visited index inline;
only a second distinct index allocates and initializes `vec![false; backend_count]`.
Rust Vec<bool> stores boolean elements, not packed bits. Thus a healthy primary,
zero budget and one rejected attempt need no tracker allocation, but fallback
initialization scales with the complete backend-list length even when only two
candidates are tested. The earlier lazy-first-index optimization remains useful.

ChCore in `lb/compiled_selector.rs` requests at most 256 distinct candidates.
ConsistentHashRing::select accepts other budgets, and repeated virtual nodes do
not consume that budget. A membership representation must handle all those
repeated probes; measuring only successful distinct insertions misses work.

Candidates include retaining the byte vector, a lazily allocated packed-word
bitmap, or a bounded sparse prefix before dense promotion. A bitmap reduces
element storage/initialization but changes membership operations. A sparse prefix
can avoid allocation for short fallback walks while adding repeated scans and
stack/container footprint. Compare actual costs before choosing a representation
or threshold. No production fleet-size or fallback-frequency distribution has
been established, and the current small-fleet inline path is already allocation-free.

Original indices are not dense ring-address IDs. prepare_ring filters non-Inet
entries and takes at most MAX_CH_BACKENDS retained Inet entries, then maps each
address to its last retained original index. Duplicate addresses and non-Inet
gaps matter. The full backend vector is retained; the cap is neither a bound on
original index values nor a count of unique addresses. Sizing a tracker from the
distinct-address count without an explicit mapping would be incorrect.

## Acceptance for a later implementation

- Preserve exact candidate and health-predicate order, duplicate suppression,
  distinct-attempt budget, weighted ring traversal, last-retained duplicate-address
  mapping, and termination after all distinct reachable addresses are rejected.
  Do not turn the budget into a raw vnode-step cap or assume every caller uses 256.
- Retain zero tracker allocation through the first candidate. Preserve membership
  across promotion, boundary word/index values, non-Inet gaps, empty/non-Inet-only
  inputs and accepted original indices beyond a compact ring count. Keep returned
  winners borrowed from the same ring generation and final caller Arc ownership.
- Measure complete ring selection and integrated ChCore selection, separating
  construction, health-predicate work and final owner acquisition. Include healthy
  primary, early fallback, many/all rejected candidates, repeated weighted vnodes,
  sizes around 128, larger fleets, sparse indices and varied budgets. Record
  allocation calls/requested bytes, initialization, latency and stack/container
  footprint; do not infer physical heap or throughput from logical element counts.
- Differentially compare winner and the ordered predicate-call trace against the
  current implementation. Exercise zero/one budgets, exact exhaustion, fallback
  beyond the first few candidates and failed complete walks. Preserve existing
  allocation and termination regressions or replace them with stronger coverage.
- Keep scratch local to the selection. No shared mutable tracker, pool, cache,
  request tracing, new metrics, eligibility snapshot change or ring/weight redesign.
  Require an exact edit allowlist, independent IMPL plan/final review, focused
  selector tests and project checks after the user selects the direction.

## Related evidence

The original [visited-storage guard](../../skills/04-review/lb/regression-guards/consistent-hash-ring-seenset-no-heap.md)
and [distinct-budget guard](../../skills/04-review/lb/regression-guards/consistent-hash-duplicate-node-budget.md)
define the current behavior. This proposal concerns fallback scratch, not the
[selected-slot reverse lookup](backend-01-selected-slot-lookup.md) or
[runtime eligibility lookup](backend-02-runtime-eligibility-lookup.md).
Fresh baseline verification passed 40 selection tests and eight compiled-selector
tests. Those validate current behavior, not a replacement or its performance.
