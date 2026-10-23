# Wasm URL module buffer transfer

Status: deferred copy-versus-capacity decision. No runtime change.
Source baseline: 0ada94e4b; locked bytes version: 1.12.1.

## Source evidence

`vm_manager.rs::fetch_url_bytes_with_permits` collects a bounded response through
the canonical `util/http_body.rs` helper, rejects truncation, then returns
`body.to_vec()`. The copy creates a separate payload allocation for nonempty
modules. Neighboring OCI `get_authed` and guest callout consumers instead use
`body.into()`; their ownership and admission contexts differ from URL loading.

The collector owns a Vec, reserving `min(8192, cap)` on the first nonempty chunk,
then growing as needed and converting it to Bytes. The locked dependency's
consuming Bytes-to-Vec conversion can recover unique backing storage and its
capacity. Shared or other backing representations can copy; do not generalize
this into an allocation-free guarantee for every Bytes value. Its implementation
also handles backing offsets, so avoid blanket claims about all internal copies.

URL loading wraps the returned Vec in `ArtifactBytes`, whose Arc-owned Payload
retains that Vec. The ordered blocking preparation closure owns the artifact
through SHA-256 validation, compatibility analysis and Wasmtime compilation.
Moving the collector allocation would therefore retain its spare capacity during
that work, whereas copying temporarily overlaps payloads but compacts the returned
storage. Preparation admission and cancellation ownership remain necessary; OCI
cache capacity charges do not automatically account for this non-OCI payload.

## Local capacity probe

A standalone probe linked against an existing locked-dependency build artifact
started with Vec::new, reserved 8192 for nonempty input, resized with ASCII `x`,
converted to Bytes, then compared `body.to_vec()` with `Vec::from(body)`. It
asserted equal contents before reporting these capacities:

| Length | Collector-like capacity | Copied Vec capacity | Consumed Vec capacity |
| --- | --- | --- | --- |
| 0 | 0 | 0 | 0 |
| 8 | 8192 | 8 | 8192 |
| 8192 | 8192 | 8192 | 8192 |
| 8193 | 16384 | 8193 | 16384 |

These are local capacity observations for synthetic inputs, not stable allocator
size classes, real network chunking, physical heap measurements or throughput
results. They establish a storage tradeoff, not a preferred implementation.

## Decision and acceptance

Retain the copy, authorize consuming transfer, or evaluate measured compaction.
Compare allocation/copy work, transient overlap, retained capacity and preparation
latency for tiny and large modules, varied chunk sizes, concurrent loads and
cancelled callers whose blocking work remains live. A shrink threshold would add
policy and work; it is not automatically beneficial.

Preserve streaming caps, exact-cap EOF and overflow rejection, URL/TLS/egress
policy, fetch permits and retries, redacted errors, digest and compatibility
validation order, blocking-work admission and cancellation release. Do not share
unverified artifacts across authorization scopes or modify OCI cache charging,
guest callout storage, inline/file sources, or main proxy response ownership as
part of a URL conversion change. No new cache, switch or body-storage API is
authorized by this task.

Any selected implementation requires a frozen scope, independent IMPL reviews,
project checks and focused successful/error/cancellation coverage. Existing URL
fetch and collector tests are starting evidence, not proof of a future transfer
strategy. No runtime change or performance claim is included here.

Relevant original knowledge:
- skills/04-review/cpu-memory/regression-guards/wasm-artifact-response-streaming-cap.md
- skills/04-review/wasm/intentional-design/bounded-oci-artifact-memory-cache.md
- skills/04-review/wasm/regression-guards/preload-claim-before-owned-inputs.md
