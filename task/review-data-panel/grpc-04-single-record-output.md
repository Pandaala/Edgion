# gRPC single-record callback output

Status: deferred user decision. Source baseline: `1c88e25c0`.

## Evidence

In `edgion-gateway/src/plugins/http/grpc_json_transcoder/response.rs`, streaming
feed decodes each completed protobuf message into a bounded JSON Vec. It resolves
the canonical RawWindow with Replace(Bytes::from(json)), copies the replacement
into a callback-local CappedWriter, then starts the next window generation.
The writer is acquired lazily, so empty or incomplete callbacks allocate no such
writer. The first completed message still requests the fixed 1 MiB callback buffer,
even when it is the only record completed in that callback. At return, into_bytes
compacts the callback output. This is in addition to the per-message codec writer.

A single-record handoff might avoid this callback aggregation buffer and copy.
The frequency and net CPU/allocation benefit have not been measured. Multiple
records can complete in one upstream chunk; this is not a one-message-per-callback
contract. Empty protobuf messages also produce nonempty JSON envelopes.

RawWindow resolution is not unconditional allocation passthrough. Its replacement
normalizer attempts Bytes::try_into_mut and retains only unique exact-capacity
buffers; other representations are copied. The resolved Bytes must remain admitted
under replacement/resident limits. A candidate must inspect that normalization and
any subsequent Bytes-to-Vec conversion rather than assume either is zero-copy.

## Ownership and limits

The existing loop independently enforces the protobuf message cap, per-message
JSON cap, 1 MiB aggregate callback cap and 256 completed records per callback.
RawWindow retains original slices and resolves each generation before start_next;
that lifecycle must remain authoritative. An optimization must handle a second
record without losing the first or changing ordering, failure behavior or limits.

The conversion plan accepts Vec output and Reservation charges its capacity, not
length. Bytes::from_owner keeps the reservation alive until downstream references
are dropped. Replacing this with an unaccounted Bytes buffer would weaken output
credits and release timing. Static terminal/error output must still obey its
existing reservation ownership; using Bytes::from_static alone is not an equivalent
shortcut around that owner. Unary response conversion has separate storage and
terminal-status requirements and is outside this candidate.

## Alternatives for user decision

- Keep the current bounded aggregation and measure actual single/multiple-record
  callback distributions, fragmented traffic, allocation counts and copied bytes.
- Investigate a callback-local first-record representation transferred on a
  single-record return, with capped aggregation only after another record arrives.
  Prove the ownership conversions and fallback costs against the locked Bytes and
  RawWindow implementations, including replacement admission and compaction.
- Consider a narrowly reviewed output-owner API change only if the existing Vec
  boundary prevents useful reuse. Account for actual resident capacity and preserve
  the reservation lifetime; do not widen this into a general response cache or a
  second window implementation.

No representation or implementation is selected. Do not remove RawWindow resolve,
expand any cap, retain a writer across callbacks, or bypass output credits to make
the fast path easier. Revisit the original conversion memory ledger with the peak
first-record/fallback overlap, including a failing later record.

## Acceptance before implementation

Compare emitted bytes and errors for NDJSON and SSE, empty/fragmented frames,
exactly one and multiple completed records, later incomplete frames, malformed
later records, 256/257 records, and independent message/aggregate/output caps.
A failing later record must not cause the first record to be published early.
Verify terminal success/error/missing-status handling, deadline/cancellation,
replacement failure, generation transitions and output credits retained through
clones until the last output reference is released.

The current callback-writer construction tests intentionally observe the existing
lazy strategy. An approved redesign needs a distinguishing first-record/fallback
observer or allocation evidence while preserving all behavioral cases; do not just
weaken their expected counts. Measure peak live capacity, allocations and copy cost
for small, near-cap and coalesced outputs. Do not infer a throughput improvement
from source-level buffer removal alone.

Relevant sources are response.rs, budget.rs, transport.rs, the canonical
`edgion-gateway/src/sse/window.rs`, and the original gRPC admission/outcome ledger
under `skills/04-review/h2-grpc/regression-guards/`. This differs from body-05's SSE
line metadata proposal. Fresh baseline checks are recorded in INDEX; no benchmark
or runtime change accompanies this task.
