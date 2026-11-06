---
name: guardrail-callback-output-capacity
description: Use when reviewing Guardrail SSE callback output allocation, rollback reservation, and callback-output-limit failures.
knowledge_type: regression-guards
---

# Guardrail callback output is reserved to its admitted bound

## Conclusion and scope

`GuardrailResponseProcessor` in `edgion-gateway/src/ai/guards/semantic/response.rs`
starts every response callback with an unallocated `BytesMut`. Output capacity is
reserved only through `reserve_output`, which admits the requested total against
`MAX_GUARDRAIL_CALLBACK_ROLLBACK_BYTES` before allocating, grows directly to that
exact bound (never amortized doubling), and re-checks the resulting capacity.

Reservation points use bounds already known at that moment:

- after a window decision, `reserve_rollback` admits the inclusive rollback bound
  (`rollback_bound`: emitted output + proposed decision + remaining raw records)
  but reserves only emitted output + the proposed decision. Remaining records are
  usually retained in the window rather than emitted, so reserving them would ship
  up to a batch of unused capacity downstream with the frozen chunk;
- `append_output` reserves the exact resolved length before copying, so a failed
  append copies nothing;
- the fail-open path in `fail_dependency` reserves the released window plus the
  untouched record tail in one step.

The callback limit itself, every `callback-output-limit` failure path, fail-open
ordering, `chunk = None` for empty output, and disabled forwarding through
`CanonicalRecordBatch::into_raw_bytes` are unchanged. This is independent of the
RawWindow admission model in [window capacity admission](window-capacity-admission.md).

## Rationale and evidence

Previously `output_buffer()` allocated the full 2,162,688-byte limit on every
callback. A retained-only batch emits nothing, so each upstream chunk paid one large
allocation and free; a small emitted event froze the whole backing into the
downstream `Bytes`, inflating retained memory per concurrent stream far beyond the
delivered bytes.

## Alternatives and limits

Reservations are exact rather than amortized, so a callback that makes several
decisions, or fails open after a decision, grows the buffer once per later
reservation by allocating the new exact bound and copying. The old and new blocks
coexist briefly during that copy (each at most the callback limit), so the
transient per-callback peak is below twice the limit. This deliberately departs
from the original task wording "no overlapping realloc peaks": that wording cannot
be met without reserving the full limit up front, which is the defect being fixed,
and `BytesMut::reserve` would double past the bound. The departure was stated in
the fix impact analysis on 2026-11-06. Admission of the full rollback bound is
unchanged, so no growth can exceed the limit. Pooling, response BodyStorage, or a
shared output arena were not introduced.

## Verification and re-evaluation

Verified on 2026-11-06: `cargo test -p edgion-gateway --lib semantic` (88 passed)
and `--lib guardrail` (9 passed), including
`callback_output_is_reserved_to_its_admitted_bound` (no allocation on rejected
admission, reservation below the limit for small output, exact growth with byte
preservation, exact-size reservations, oversized append rejected without copying)
and the updated
`rollback_reservation_is_inclusive_and_checked`.

Re-evaluate if callback output gains another append site, if decisions can emit
bytes not counted by the rollback bound, if the rollback bound stops including
remaining raw bytes, if multi-decision callbacks become common enough that
growth copies matter, or if `BytesMut` reservation semantics change.

## Resolution references

- `guardrail-03-callback-output-capacity` — corrected on 2026-11-06.
