# Full-range request-body snapshot reuse

Status: deferred ownership decision for the user.
Source baseline: `9b1c0285f`. No runtime change.

## Evidence

`BodyStorageView::range_snapshot` in `edgion-gateway/src/request_body/storage.rs`
acquires the current immutable snapshot, rejects an offset beyond its length,
clips the saturating end offset, and constructs a new snapshot from
`snapshot.slice(offset..end)`. This occurs even when offset is zero and the
clipped end equals the complete logical length. The full raw-range scan-budget
check follows construction and precedes output collection.

`BodySnapshot::slice` builds a segment Vec and clones SourceRange Arc handles or
slices Inline Bytes. `from_segments` creates another normalization Vec, sums
checked lengths and installs a fresh SHA-256 cell; range_snapshot adds an Arc
owner. The selected body payload is already shared, not copied by this metadata
construction. Empty ranges do not allocate segment payloads. Partial ranges need
their own logical projection and are outside the proposed full-range shortcut.

Both bounded text and raw range reads use this function. The active DSL path is
BodyRead -> DslBodyOps::read_text_range -> RequestBodyState -> storage. The active
raw path is SessionAdapter::read_request_body_prefix, which reuses an existing
complete capture through RequestBodyState::read_range. A complete body no larger
than the prefix limit exercises the whole-range case. The adapter freezes
mutation first, reads current logical edits, and reports completeness from the
cached length. The separate uncaptured-prefix path is not this candidate.

`scan_snapshot` transfers its owned snapshot into synchronous small-memory
scanning or holds it across scan-lane admission and blocking execution. Returning
the existing snapshot for a whole-range read could remove segment reconstruction,
normalization and the new snapshot allocation. It would retain the original
snapshot Vec capacity and digest cell, rather than independent normalized
metadata. The distinction matters when another permitted owner publishes a new
generation or a cancelled scan worker outlives its waiter. The frozen WAF caller
has a narrower mutation lifetime than the generic storage API.

The payload backings are already shared in both designs. No extra body copy is
claimed, and no allocation, capacity, peak-memory or throughput measurement has
been made. Original snapshot capacity need not equal its segment count: normalizer
input capacity and merging can differ from the final logical shape. Reuse is not
proven to reduce retained metadata in every case.

## Decision

Retain the current independent range snapshot, or evaluate a private whole-range
reuse branch after clipping and validation. Do not generalize this into borrowed
worker state, partial-range reuse, a new cache, or a new BodyStorage owner.
Prove valid-snapshot invariants for skipped slicing/normalization checks before
removing them; existing private malformed-snapshot test behavior must be assessed
explicitly rather than silently treating those cases as production input.

This is separate from [body-07](body-07-normalized-segment-ownership.md), which
concerns finishing mutation builders without a second normalization pass, and
[body-03](body-03-mutation-snapshot-retirement.md), which concerns destruction
under the publication lock. Neither deferred decision authorizes this shortcut.

## Acceptance for an approved implementation

Preserve invalid-offset rejection, saturating end clipping, empty/EOF ranges,
full raw-range budget preflight before output allocation or lossy-text overflow,
segment/chunk fetch boundaries and exact scan accounting. Keep isolated-range
UTF-8 decoding, output limits, early-stop refunds and all error mappings.

Preserve immutable generation selection, replay isolation, physical/segment
bounds, scan permits, cancellation checks and worker ownership after waiter drop.
Raw prefix reuse must retain current logical edits, mutation freezing, inspected
flags, completeness reporting, stage eligibility and no remainder materialization.
Do not add hot-path logs, metrics, response aggregation or a second body cache.

Compare current and candidate allocation requests, peak overlap and retained
metadata capacity for empty, memory, disk and mixed snapshots, large segment
counts, merged generations and whole versus partial ranges. Include concurrent
mutation and cancellation while waiting for admission or executing a worker.
Distinguish metadata savings from payload backing retention. Keep final raw Bytes
and bounded text String ownership unchanged unless separately reviewed.

Use IMPL with a frozen runtime allow-list and fresh storage, DSL Body and WAF
prefix verification. Existing integration fixtures remain unchanged. Tests of the
current implementation do not approve the proposed ownership change.

## Sources and verification limits

- `edgion-gateway/src/request_body/storage.rs`: range_snapshot, BodySnapshot,
  scan_snapshot and scan_snapshot_owned.
- `edgion-gateway/src/request_body/state.rs` and `state/enabled.rs`: forwarding
  and known-empty behavior.
- `edgion-gateway/src/plugins/http/dsl/lang/body_ops.rs`: bounded text policy.
- `edgion-gateway/src/plugins/runtime/session_adapter.rs`: read_request_body_prefix.
- [DSL body architecture](../../skills/01-architecture/02-gateway/dsl-engine.md)
- [Storage I/O ownership](../../skills/04-review/http-body/regression-guards/request-body-storage-io-deadline.md)
- [Body read contract ownership](../../skills/04-review/http-body/intentional-design/request-body-read-contract-ownership.md)

This task records a source-backed optimization candidate and its unresolved
ownership tradeoff. It changes no runtime code and reports no candidate benchmark.
