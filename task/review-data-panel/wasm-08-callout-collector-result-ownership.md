# Wasm callout collector result ownership

Status: deferred for user decision. Type: performance investigation.
Inspected baseline: `2cf771385`. Locked bytes version: 1.12.1.

## Evidence

`wasm/callout.rs::execute_callout_attempt` receives a bounded response from
`util/http_body.rs::stream_body_capped`, rejects truncation, and consumes its
Bytes into the Vec held by CalloutResponse. The canonical collector accumulates
in a Vec, initially reserving min(8192, cap) only after nonempty data, then returns
Bytes::from(buf). No clone or slice intervenes before this callout conversion.

In locked bytes, a Vec with spare capacity becomes a Bytes with an allocated
Shared metadata owner. The unique consuming conversion frees that metadata and
recovers the original allocation and capacity. Equal length/capacity takes the
boxed-slice path instead. The conversion implementation handles offsets and
shared owners generally; do not claim every Bytes-to-Vec conversion avoids copies.
This specific unique, unsliced path does not need a separate payload allocation.

A standalone local probe linked against the locked dependency counted only
Vec-to-Bytes-to-Vec conversion, excluding input construction and final output
destruction. It checked content, capacity and nonempty backing-pointer identity:

| Length | Input/output capacity | Allocations | Deallocations |
| --- | --- | --- | --- |
| 0 | 0 | 0 | 0 |
| 8 | 8192 | 1 | 1 |
| 8192 | 8192 | 0 | 0 |
| 8193 | 16384 | 1 | 1 |

These synthetic observations support a metadata-work candidate, not a workload
bottleneck, payload-copy bug, physical heap bound or throughput improvement.

## Decision alternatives

Retain the current interface unless measured callout frequency and body shapes
justify changing it. Alternatively, return Vec from the single canonical
collection loop and retain a Bytes adapter for consumers that need shared bytes.
A targeted Vec entry point avoids changing CalloutResponse but expands the
shared helper interface. Do not duplicate the capped streaming loop.

Changing CalloutResponse.body to Bytes is a different design: it reaches host ABI
range snapshots, reentry isolation and shared backing-storage lifetime.
It is not an incidental type substitution justified by this metadata observation.

The current consumer audit also finds OCI get_authed consuming into Vec, while
outbound responses retain Bytes and DSL/OIDC/bulk-receipt/token readers parse or
decode borrowed slices. URL module loading explicitly copies and has a separate
[copy-versus-capacity decision](wasm-07-url-module-buffer-transfer.md). Do not
silently include these consumers or their ownership decisions in a callout fix.

## Acceptance for a selected implementation

Freeze exact consumers and API changes after independent IMPL plan review.
Preserve the 256 KiB callout streaming cap, exact-cap EOF and overflow rejection,
read-error behavior, header admission/order, endpoint policy, request framing,
absolute deadlines, permit release, callback token and frame cleanup. Failure
callbacks retain the token with empty headers and body. Keep independent snapshots
before guest allocator reentry; no live HostState borrow may cross guest execution.
Main proxy responses remain streaming, with no new BodyStorage or cache.

Measure metadata allocations separately from body accumulation, guest snapshots
and copies, retained capacity, cancellation lifetime and complete callout latency.
Compare empty, small, exact-capacity and growth-boundary bodies with varied chunks
and concurrent calls. Preserve pending/empty/error zero-allocation collector
behavior, no polling after overflow, stream destruction and failure classification.
Existing collector and callout tests are starting coverage, not proof of a new API.

Fresh unchanged-code checks passed five collector and eleven callout tests
(serial callout execution, including existing loopback fixtures). No runtime
prototype, full suite or workload benchmark is included.

Original knowledge:
- skills/04-review/wasm/regression-guards/buffer-range-snapshot-before-guest-allocation.md
- skills/04-review/wasm/regression-guards/request-execution-lifecycle.md
- skills/04-review/cpu-memory/regression-guards/wasm-artifact-response-streaming-cap.md
