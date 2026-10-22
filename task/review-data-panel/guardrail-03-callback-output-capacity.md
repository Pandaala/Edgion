# Guardrail callback output capacity

Status: deferred for user decision. No runtime change.
Source baseline: `c3f1be5c2`.

## Evidence and decision

`GuardrailResponseProcessor::output_buffer` requests a BytesMut capacity of
2,162,688 bytes (2.0625 MiB): the 1 MiB RawWindow raw limit, 1 MiB pending SSE
event limit and 64 KiB canonical source chunk limit. It rejects actual capacity
above that bound. Active available and unavailable `process_batch` paths allocate
this buffer before processing their records. An available batch may only retain
records below the decision threshold and return no output, dropping the unused
buffer. Decision awaits can retain it alongside window, batch and callout state.
Non-SSE and missing-EOS-source failure paths also use the helper. A missing
non-EOS batch bypasses it; disabled processing uses `into_raw_bytes`, which has
its own output allocation. This is not an allocation on every callback.

Nonempty output is frozen into Bytes. In locked bytes 1.12.1, BytesMut::freeze
transfers the backing allocation; From<Vec<u8>> retains spare capacity in Shared.
A small emitted chunk can therefore keep the large output backing after callback
return, until that owner and any shared handles are released. This establishes
visible backing ownership, not allocator RSS, downstream retention duration or
measured throughput. The current capacity test explicitly asserts the full cap.

Compare the current fixed reservation with a lazy, bounded callback writer.
Lazy creation could avoid unused output storage; smaller bounded growth could
reduce backing retained by small outputs. These are separate choices: allocating
the same fixed buffer on first append avoids empty work but does not compact small
outputs. A growing writer may introduce reallocations/copies and overlapping old
and new allocations. Compacting at publication adds a copy. No option has an
established net benefit, and a length-only check is not a replacement heap bound.

## Boundaries and acceptance

- Preserve checked inclusive rollback accounting for emitted output, the proposed
  decision and remaining raw records. Fail-open must release the current window
  and untouched tail in order; failure must never expose partial replacement or
  lose original bytes. Preserve summaries and disabled forwarding.
- Admit capacity before allocation, including growth peaks and simultaneous old
  and new storage. Keep the independent RawWindow, batch, callback-output,
  replacement, callout and held-head bounds. Do not substitute logical length for
  capacity or introduce response BodyStorage, pooling or full-response aggregation.
- Preserve semantic admission and decision timing, call count/order, held-head
  work reservation, release/reject/failure ordering, terminal ownership, EOS and
  cancellation cleanup. Deferring the current capacity check must not silently
  alter failure policy or precedence; review any such difference explicitly.
- Verify available empty/retained/raw-only batches, partial source records,
  unavailable source, non-SSE/missing EOS, disabled forwarding, pass/replace/reject,
  fail-open/closed and cancellation while dependency admission is pending. Cover
  several decisions per callback, exact/exceeded limits and rejected retain/retry.
- Compare allocation calls and peak/live capacities separately for empty, small
  and near-limit output, concurrent pending decisions and slow downstream release.
  Check byte identity and terminal outcomes before measuring performance. Existing
  capacity, rollback and source-handoff tests are baselines, not proof that a new
  writer preserves the contract.

Use IMPL with an exact implementation allow-list after user selection. No runtime
optimization is authorized by this deferred task alone.

## Sources and scope

- `edgion-gateway/src/ai/guards/semantic/response.rs`: output_buffer,
  process_batch, append_output, fail_dependency and on_chunk.
- `edgion-gateway/tests/unit/ai/guards/semantic/response_test.rs`:
  rollback_reservation_is_inclusive_and_checked and lifecycle/source tests.
- Locked bytes 1.12.1: BytesMut::with_capacity/freeze and Bytes::from(Vec<u8>).
- `skills/04-review/sse/regression-guards/window-capacity-admission.md` and
  `skills/04-review/sse/regression-guards/canonical-chat-block-classification.md`.

This is distinct from [window statistics](sse-01-incremental-window-statistics.md),
[held headers](guardrail-01-held-replacement-header-storage.md),
[decision snapshot encoding](guardrail-02-decision-snapshot-encoding.md) and
[transcoder output ownership](grpc-04-single-record-output.md).
Fresh existing tests passed 25 semantic response and 12 canonical SSE cases.
No prototype, allocation measurement, live transport or benchmark ran.
