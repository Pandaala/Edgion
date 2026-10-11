---
id: ARCH-038
status: candidate
kind: architecture-debt
priority: P3
partition: PART-344
analyst: /root/analyze_conversion
independent_verification: VERIFICATION-ARCH-038.json
---

# Snapshot only the requested Wasm ABI buffer range

## Current ownership and concrete cost

`host_abi.rs::select_buffer` (767-784) clones complete HostState Vec buffers
(request body, configuration and current callout response). PROXY_GET_BUFFER_BYTES
(1848-1872) selects before validating start/computing its range. A one-byte read
from B bytes, or an invalid offset, first copies B bytes. Unsupported setter
classification (1884-1889) also selects/clones merely to identify the buffer type.
The response-chunk variant already clones a shared Bytes handle.

A guest performing N small request-body reads repeats approximately N times B
selection bytes, in addition to the necessary selected payloads. This static
cost is distinct from initial plugin.rs staging and ARCH039 idle retention.
No allocator/RSS/throughput or production workload was measured.

## Bounded improvement and alternative

Borrow the selected source only while checking the unsigned offset/range; snapshot
only that range for Vec-backed sources. Release HostState's borrow before
write_to_guest invokes the guest allocator. Guest code may re-enter, so keeping
an ordinary HostState slice borrowed across allocation is not the simplification.
Keep shared Bytes ownership/slicing for response chunks. Classify known read-only
setter targets without materializing payloads. Partial selection becomes O(R)
copy for the returned range; whole-buffer reads still copy requested bytes.

Converting every source to shared immutable storage would require separately
reviewed request copy-on-write semantics. The range snapshot keeps current
representations. The owned-request selection test mirrors current shape; mock
convenience cannot require complete buffer duplication on the hot path.

## Invariants and future verification

Preserve canonical request BodyStorage, complete terminal guest request callback,
unsigned start/max_size including -1 whole-buffer behavior, empty/slice/error
statuses, atomic staged request mutation under physical/work caps, read-only
current response chunks and UNIMPLEMENTED response mutation. Retain guest/ABI/
callout/config/fail-mode caps, canonical egress and observability. No cache,
whole-response API, tracing, metrics mechanism or session capability is added.

Verify partial/empty/invalid/whole-buffer reads, snapshot stability across guest
allocation/re-entry, known/unknown setter statuses and unchanged mutation. A
structural assertion should establish small-range copying, not just output.
Original whole-body callback, ABI retention, runtime-hardening and response shared
storage decisions plus actual tests were read. REPORT-PART-344/345/346.json record
full primary hashes and exact support scopes. Static only, no execution/mutations;
independent verification pending.
