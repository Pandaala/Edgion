# Semantic window fragment projection

Status: deferred for user decision. No runtime change.
Source baseline: `b2470718d`.

## Evidence and decision

`RawWindow::visit_bucket` clears its admitted token scratch and copies every
fragment's ranges into it before UTF-8 validation or JSON string decoding.
A fragment containing one range can already be contiguous in the canonical
SliceChain, including coalesced storage. That case could potentially borrow
bytes instead of copying them. Statistics, full snapshots, individual bucket
materialization and overlap preparation all reach this visitor.

The existing `SliceChain::range` is not an allocation-free probe: it returns
borrowed bytes when contiguous, but allocates a Vec when a range spans slices.
Using it blindly alongside token scratch would add storage outside the current
admission calculation. Multiple ranges may also require joining before UTF-8
validation; validating each physical slice independently changes semantics.

Compare these choices before implementation:

- Retain current assembly. It uses one reusable, admitted scratch owner per
  operation and has no speculative contiguous-range lookup.
- Add a nonallocating projection in the canonical SliceChain owner, initially
  for one contiguous range, with the existing assembly fallback. Keep the same
  scratch reservation and admission sequence. This can avoid payload copies but
  does not remove the scratch allocation; failed probes can add traversal work
  for fragmented inputs. Do not duplicate chain boundary logic in a plugin.
- Change scratch sizing or allocation according to physical fragment storage.
  This requires a separate admission-semantics decision: `token_scratch` admits
  the maximum token length before validation, and later materialization/overlap
  gates include actual scratch capacity. Removing allocation or charging less
  capacity can change which operation succeeds or which error appears first.

No choice is selected, and no prototype or measurement establishes a net gain.
This differs from [incremental statistics](sse-01-incremental-window-statistics.md),
which adds generation-owned summaries; [fallback handoff](sse-02-fallback-record-handoff.md),
which transfers raw records; and [event envelopes](sse-03-canonical-event-envelope.md),
which changes canonical event construction. It does not authorize any of those
changes or reopen the existing binary-message borrowing path.

## Acceptance before implementation

- Preserve fragment and bucket order, UTF-8 validation across physical slice
  boundaries, JSON escaping/surrogate validation, error categories and precedence,
  semantic degradation, and original raw bytes. Do not cache decoded response text.
- Preserve every raw/resident/metadata/snapshot/overlap/combined capacity bound,
  checked arithmetic and admission before allocation. Count simultaneously live
  storage; logical token length alone does not establish heap residency.
- Preserve failed retain atomicity, generation transitions, release/replace/discard,
  context derived from emitted text and terminal EOS cleanup. No response
  BodyStorage, second semantic window, full-response aggregation or body-phase logs.
- Compare empty and multiple ranges, contiguous and fragmented slices, coalesced
  storage, split multibyte UTF-8, escaped JSON and malformed tokens. Include
  competing capacity failures, exact-cap and cap-minus-one cases, repeated stats,
  full snapshots, individual materialization and overlap consumers.
- Measure copied bytes, lookup traversal, allocation calls and peak/live capacity
  separately for small contiguous fragments and fragmented workloads. Preserve
  behavior before claiming throughput improvement.

After user selection, use IMPL with an exact implementation allow-list. This task
records the decision boundary and grants no runtime implementation permission.

## Sources and verification

- `edgion-gateway/src/sse/window.rs`: token_scratch, visit_bucket, scan,
  materialize_all, materialize_bucket and prepare_overlaps.
- `edgion-gateway/src/sse/chain.rs`: range and for_each_range.
- `skills/04-review/sse/regression-guards/window-capacity-admission.md` and
  `skills/04-review/sse/regression-guards/window-emitted-context.md`.

Fresh existing gateway library tests passed 94 cases under `sse::` and 25 under
`ai::guards::semantic::response` (119 distinct identities). These establish the
current behavior only. No new tests, runtime edit, prototype, allocation probe,
full suite, live transport or benchmark ran. Independent plan review passed.
