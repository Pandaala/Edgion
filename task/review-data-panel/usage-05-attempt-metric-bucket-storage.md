# Attempt metric bucket storage

Status: deferred representation decision for user review. No runtime change.
Source baseline: `36010cec8`.

## Evidence

`AiAttemptRuntimeState::commit_ledger` aggregates input/output token totals in a
Vec<AiAttemptTokenTotals>, searching by provider and usage source. Entries contain
Copy scalars; existing buckets update in place, new buckets append in first-seen
order. Empty storage is lazy, but first insertion needs heap storage. Three
provider variants and four usage-source variants give a conservative type-domain
bound of 12 distinct pairs, not evidence that all pairs occur in production.

Aggregation precedes the attempt-log count and byte caps. Dropped log entries
still contribute to metric and charged-token accounting. The eight-entry log or
dispatch limit therefore must not be repurposed as a bucket-storage invariant.
The existing oversized-log regression explicitly verifies retained metric totals.

Checked arithmetic is not all-or-nothing across these owners: request-charge
overflow does not prevent independently valid metric/adaptive updates; bucket
overflow preserves that bucket's prior value and marks accounting overflow.
Unknown/unobserved attempts do not erase previously known buckets. Snapshot reuse
and incremental log-size accounting are separate deferred decisions.

`ai/accounting/finalize.rs` borrows the bucket slice and emits Reported/Estimated
totals in the ever-dispatched branch, even when the request total is unavailable.
Its separate non-attempt fallback can emit directly from usage. The summary owns
the once-only finalization sentinel; this storage proposal changes neither path.

The runtime sits inside lazily boxed AiHttpState. Ordinary request contexts start
with no AI allocation. Inline storage would enlarge that AI allocation, including
AI requests that never populate a bucket; it must not be described as free stack
storage or as necessarily improving total memory.

## Decision alternatives

- Keep the lazy Vec and bounded linear search if common request sizes do not
  justify a larger inline owner or indexed representation.
- Use a small inline representation with explicitly bounded overflow behavior.
  Compare its common-case allocation saving against the enlarged AI state and
  transition cost. Preserve first-seen iteration and all bucket values.
- Use an enum-indexed representation with explicit occupancy and iteration rules.
  Prove complete key coverage and make enum growth a checked maintenance gate;
  do not index by unchecked casts or silently drop a new pair. Assess any change
  from first-seen emission order before selecting this design.

No alternative is selected. Do not combine this with a new metric family, label,
sampling policy, accounting snapshot, limiter, cache or global accumulator.

## Acceptance before implementation

Cover empty state, one bucket, repeated updates, distinct provider/source pairs,
mixed reported/estimated/unknown/not-observed attempts, zero values, all checked-add
overflow branches and prior-bucket retention. Preserve charged totals and adaptive
updates independently of log truncation; exercise both count and byte caps.
Keep known metrics when unresolved usage removes the request total, once-only
finalization, the non-attempt fallback and metrics-disabled behavior.

Preserve the borrowed consumer lifetime, reset/drop behavior, lazy ordinary-context
allocation, settlement ordering and canonical metric ownership. An implementation
must not impose a new bucket truncation policy or assume only presently common
pairs are representable. Existing tests are baseline evidence, not proof of a
replacement container's capacity or enum evolution.

Measure one-bucket, multiple-provider/source and no-final-token workloads. Compare
allocation calls, requested/retained capacities, AI state size, update/iteration
CPU and concurrent-request memory, including overflow transitions if applicable.
No allocation-count, throughput or RSS improvement is established by source
inspection alone. Freeze scope and obtain independent IMPL plan/final review.

## Sources and verification

- `edgion-gateway/src/ai/runtime/attempt_runtime.rs`: commit_ledger, metric_totals.
- `edgion-gateway/src/ai/accounting/finalize.rs`: final bucket/fallback emission.
- `edgion-gateway/src/ai/accounting/usage.rs`: provider and usage-source enums.
- `edgion-gateway/src/ai/context.rs`: lazy AiHttpState ownership.
- [Prepared runtime ownership](../../skills/04-review/ai-routing/regression-guards/prepared-attempt-owns-admission-input.md)
- [Accounting snapshot decision](usage-02-accounting-snapshot-reuse.md)
- [Attempt log-size decision](log-02-incremental-attempt-size.md)

Fresh unchanged-source verification passed 25 attempt-runtime, 52 usage,
13 finalizer and 36 dispatch tests (126 total). No prototype, benchmark, full
suite or live integration ran for this audit.
