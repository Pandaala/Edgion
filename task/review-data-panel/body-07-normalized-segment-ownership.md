# Normalized request-body segment ownership

Status: deferred ownership and retained-capacity decision for the user.
Source baseline: `fb27a5427`. No runtime change.

## Evidence

In edgion-gateway/src/request_body/storage.rs, replace_literal and replace_spans
assemble their candidate Vec through SegmentSplicer and push_normalized_segment.
Every retained source slice and nonempty replacement goes through the canonical
normalizer: empty segments are discarded, adjacent SourceRange segments merge
only for identical Arc source identity and contiguous offsets, and segment count
is bounded by MAX_BODY_SEGMENTS (4,096). Inline segments do not merge by content.

Both paths then call BodySnapshot::from_segments. That constructor invokes
normalize_segments, allocates another Vec, moves each segment through the same
normalizer, and separately sums checked segment lengths before creating a fresh
digest cell. The existing constructor also serves ordinary mutation paths that
have not already normalized their output, so removing its normalization globally
is not justified.

The initial splice Vec requests capacity from source count plus edit/match count,
capped at 4,096. Deletions and merging can leave far fewer final segments. The
second vector requests capacity from its input length, which for these normalized
paths is the final segment count. Directly retaining the splice Vec may therefore
remove an allocation and normalization walk while increasing long-lived snapshot
capacity. Initial requested capacity and bounded segment length do not establish
a hard Vec-capacity bound: growth and allocator rounding must be measured.
Moving a Segment does not copy its body payload. No allocation count, retained
heap improvement or throughput benefit has been measured.

## Decision

Keep the current normalization boundary, or design a private canonical builder
that can finish already-normalized storage without rebuilding it. Specify the
retained-capacity policy before selecting that representation. Shrinking may
reallocate; simply preserving capacity is not an unqualified memory improvement.
Any trusted normalized constructor must have an explicit invariant and restricted
callers rather than becoming a bypass available to arbitrary mutation code.

The optimization concerns the extra normalization walk and vector ownership.
Checked total-length summation must remain, unless a separate reviewed design
fuses it into the canonical builder without changing failure behavior. Avoid a
second normalization implementation, unsafe compaction or a new body cache.

This choice is separate from [snapshot retirement](body-03-mutation-snapshot-retirement.md),
which concerns dropping replaced owners under a mutation lock, and from literal
pattern preparation. Neither decision authorizes changing the other boundary.

## Acceptance for an approved implementation

Preserve empty-segment handling, source identity and adjacency rules, checked
offset/length arithmetic, the segment limit and immutable physical/owner cap.
Keep validation and error precedence, empty/no-match no-ops, generation conflict
and frozen-state handling, fresh digest state, and atomic publication. Failed
mutations must leave the old complete body visible. Old readers, scan jobs and
replay snapshots must retain their existing source ownership and cancellation
behavior. Do not widen production visibility for tests.

Compare current and proposed construction for memory, disk and mixed sources;
many deletions; adjacent source merges; inline replacements; unchanged edits;
boundary segment counts; overflow/invalid ranges; and physical-cap rejection.
Measure allocation requests, peak overlap and retained Vec capacity separately,
including cases where a large input produces very few segments. Compare replay
bytes and scan results, then generation/freeze/failure behavior. Do not infer a
bounded heap from logical byte length or segment count alone.

An approved implementation requires a frozen scope, IMPL plan/final review and
fresh targeted verification. Existing integration fixtures stay unchanged. Keep
canonical BodyStorage ownership and add no hot-path logs or metrics.

## Knowledge and limits

- [Mutation physical cap](../../skills/04-review/http-body/regression-guards/request-body-mutation-physical-cap.md)
- [Storage I/O ownership](../../skills/04-review/http-body/regression-guards/request-body-storage-io-deadline.md)
- [DSL body architecture](../../skills/01-architecture/02-gateway/dsl-engine.md)

This records source evidence and a design choice, not an approved implementation.
Existing tests validate current behavior; no proposed builder, allocation probe
or performance benchmark was run for this audit.
