---
name: buffer-range-snapshot-before-guest-allocation
description: Use when reviewing Wasm ABI buffer selection, GET ranges, setter classification, or guest allocator reentry.
knowledge_type: regression-guards
---

# Buffer ranges own their snapshot before guest allocation

## Conclusion and scope

In `edgion-gateway/src/plugins/http/wasm/host_abi.rs`, `select_buffer` borrows
recognized source storage without copying payload. `BufferSource::snapshot`
validates the requested unsigned range, then owns only that range before
`write_to_guest` calls the guest allocator. Borrowed sources (including plugin configuration) copy the selected
bytes with fallible reservation; response chunks use a shared `Bytes::slice`.
The returned `BufferSnapshot` has no borrow of HostState.

The separate legacy `proxy_get_configuration` accessor clones the immutable
per-HostState `Bytes` owner before guest allocation, avoiding a full temporary
payload copy. Construction transfers the existing Vec into Bytes; ModuleEntry
still creates separate configuration payloads per VM. Modern GET remains on the
borrowed range-copy path. Bytes adds ownership metadata and may allocate control
storage during construction or its first clone; this is not a zero-allocation callback or a measured
request-throughput improvement. Guest-memory writes still copy the payload.

GET and type-only SET classification use the same recognized-source selector.
Rejected known configuration/callout SETs return BAD_ARGUMENT without copying
payload or reading guest pointers; unknown SETs return NOT_FOUND. Response SET
returns UNIMPLEMENTED before inspecting any other argument. Request mutation
keeps its existing phase, physical size, cumulative work and ABI admission.

## Rationale and evidence

ARCH-038 identified full-source Vec clones before GET range validation and in
rejected SET classification. A small valid GET copied unrelated source bytes;
invalid offsets and type-only rejection copied bytes they never returned.
The borrowed selector separates classification from the snapshot owner, while
keeping HostState storage and mutation authority unchanged.

`write_to_guest` invokes `proxy_on_memory_allocate` (or `malloc`) before writing
the selected bytes. That guest function can reenter imports and mutate request
staging. A borrowed source slice cannot span this call: the outer GET must return
its pre-allocation range even when the allocator changes the source. Independent
range ownership provides that contract without a full-source copy.

Unsigned start and max_size preserve wasm32 `-1` whole-read behavior. Unknown GET
returns NOT_FOUND before range or guest-memory checks. Nonempty `start >= len`
returns BAD_ARGUMENT, including zero-size reads at the end. Empty sources accept
arbitrary offsets, including absent callout responses. Successful empty and
zero-size reads still invoke the guest allocator and retain its failure behavior.
Clamped end arithmetic remains saturating.

## Alternatives and limits

Validating before a full clone would remove invalid-read cost but leave partial
reads copying the complete source. Changing every source to immutable shared
storage would widen request mutation ownership unnecessarily. Holding a live
HostState reference through guest allocation would break reentry isolation.

This correction removes full-source selection copies for partial reads and
classification. Whole-buffer Vec reads still copy the requested whole buffer,
and host-to-guest memory writes still copy bytes. Initial complete request
staging and publication remain unchanged. Response sources remain read-only
current chunks; this creates no response BodyStorage or aggregation permission.
See the [complete terminal request callback contract](wasm-whole-request-body-callback-contract.md).

Recursive guest GETs can retain multiple independent snapshots concurrently.
Range-scoped copying does not establish an aggregate host-heap bound, and the
existing [ABI retention budget](../../cpu-memory/regression-guards/wasm-abi-retention-budget.md)
and guest memory/time limits are not proof of such a bound. No new read budget,
logging, metric, cache, egress mechanism or configuration knob is introduced.
No measured throughput, RSS or latency gain is claimed.

## Verification and re-evaluation

The matching `tests/unit/plugins/http/wasm/host_abi_test.rs` covers:

- small independently owned request ranges and all Vec-backed source types;
- shared response backing-pointer offsets surviving source replacement;
- real-guest unsigned ranges, known/unknown status precedence, clamping,
  empty/zero-size allocator invocation and allocator/memory failures;
- a guest allocator reentering request SET, with the outer GET returning its
  original bytes while staged request storage reflects the mutation;
- existing mutation phase, atomicity, physical-cap and work-budget regressions.

Fresh execution results belong to `impl/architecture-ARCH-038/` and must be
checked there; test definitions and historical receipts alone are not passing
execution evidence. Re-evaluate on changes to source storage, recognized buffer
IDs, GET range/status rules, guest allocation/reentry ordering, or request and
response body ownership. Any new aggregate-memory claim requires independent
accounting and evidence.

## Resolution references

- ARCH-038 — range-scoped buffer snapshot refactor; provenance and fresh review/check receipts in `impl/architecture-ARCH-038/`.
