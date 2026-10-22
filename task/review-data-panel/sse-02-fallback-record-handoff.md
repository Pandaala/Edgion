# Canonical SSE fallback record handoff

Status: deferred for user decision. No runtime change.
Source baseline: `b920e428b`.

## Evidence and decision

`CanonicalRecordBatch::into_records` consumes the batch but projects its nonempty
fallback through the borrowing `SliceChain::raw_slices` API. Sliced storage clones
Bytes handles into a new Vec; coalesced storage copies the payload into a new Bytes
and wraps it in a Vec. `WindowRecord::raw` then counts lines before the record is
marked semantically unavailable. Existing annotated records move unchanged.

`SliceChain::push` coalesces on the 257th nonempty handle and requests the caller's
byte limit as arena capacity. For this batch the limit is 1 MiB + 64 KiB. Many tiny
records can therefore produce a coalesced fallback much smaller than its arena.
The current copy releases that arena when into_records returns, before the caller
allocates its callback output buffer. This is not an unconditional payload copy
for every batch or a claim about allocator RSS.

Active Guardrail `process_batch` converts an unavailable batch to records before
output-buffer admission and `fail_dependency`. That helper first handles a held
response head, which forces failure, then selects fail-open release or fail-close
discard. A copied fallback can thus be discarded without forwarding. Disabled
processing uses the separate `into_raw_bytes` path and is outside this proposal.

Compare three choices before implementation:

- Retain the current compact copy, which drops spare coalescing capacity before
  allocating output, but pays for projection even when records are discarded.
- Add a consuming handoff for this owner. Moving handles or freezing the arena
  could avoid reference-count traffic or copying, but retains arena slack and may
  change handle-table capacity. Account for simultaneous records and output, not
  only logical byte length. Do not change the shared borrowing API by assumption.
- Defer raw-tail projection until failure handling actually needs fail-open
  output, or append from the owned batch through a bounded visitor. This could
  avoid constructing records on discard paths, but must preserve held-head and
  output-admission precedence and must not create a second fallback mechanism.

No prototype or measurement establishes the best choice. This is distinct from
[Guardrail callback output capacity](guardrail-03-callback-output-capacity.md),
which owns the later output buffer, and [window statistics](sse-01-incremental-window-statistics.md).
The already optimized into_raw_bytes materialization is not reopened here.

## Acceptance before implementation

- Preserve original byte order, record classification, checked raw admission,
  metadata limits, zero-byte records and the meaning of an empty fallback chain.
  Never substitute unavailable status for storage shape.
- Preserve input/output summaries, disabled state, terminal ownership, held-head
  failure, fail-open/closed behavior and exactly-once window cleanup. Keep existing
  callback-output admission and error precedence or explicitly review a change.
- Account for copied and moved storage, spare arena capacity, handle tables and
  simultaneous output. Do not add response BodyStorage, pooling, unbounded
  aggregation or a second semantic window. Any helper belongs to the canonical
  existing owner and must retain unaffected callers' contracts.
- Cover annotated-only, sliced and coalesced fallback, metadata/count overflow,
  empty raw records, tiny and near-capacity batches, held and committed headers,
  both failure policies and terminal-owner conflicts. Check cleanup when output
  admission or window release fails. Existing tests are baseline evidence, not
  acceptance of a new ownership scheme.
- Measure copies, allocation calls and peak/live capacities separately, including
  small payloads in large arenas and concurrent callbacks. Verify byte identity
  and terminal outcomes before drawing throughput conclusions.

Use IMPL with an exact implementation allow-list after user selection. This task
does not authorize a runtime change.

## Sources and verification

- `edgion-gateway/src/ai/protocols/openai/canonical_sse.rs`: push, into_records.
- `edgion-gateway/src/sse/chain.rs`: push, raw_slices, take_raw_slices_into.
- `edgion-gateway/src/sse/window.rs`: WindowRecord::raw and count_lines.
- `edgion-gateway/src/ai/guards/semantic/response.rs`: process_batch,
  fail_dependency, stage_head_failure and output_buffer.
- `skills/04-review/sse/regression-guards/window-capacity-admission.md` and
  `skills/04-review/sse/regression-guards/canonical-chat-block-classification.md`.

Fresh existing tests passed 13 canonical SSE, 25 semantic response and five chain
cases. No new tests, prototype, allocation measurement, live transport or benchmark
ran. Source inspection establishes the handoff and ordering described above.
