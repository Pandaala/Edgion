---
name: h1-request-body-capture-bounded-second-copy
description: Use when reviewing the second memcpy performed while retaining captured Pingora HTTP/1 request-body chunks; Edgion bounds the in-memory copy by its spill threshold, and a segmented-store refactor is not justified without profiling evidence.
status: accepted-tradeoff
finding_id: pingora-review-16
closed: 2026-08-11
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# HTTP/1 Captured Request-Body Second Copy — ACCEPTED TRADEOFF

## Conclusion

Pingora's HTTP/1 server copies each body chunk out of the reusable body-reader
buffer into an owned `Bytes`. Its reference `InMemoryRequestBodyBuffer` then
copies the chunk into an accumulating `BytesMut`. The mechanism is real, but
changing that reference implementation to `Vec<Bytes>` does not optimize
Edgion: the Gateway registers its own `BodyStorage` implementation.

Edgion's in-memory capture stage also appends into a `BytesMut`, but only until
the configured memory spill threshold. The default threshold is 128 KiB; later
chunks are cloned as refcounted `Bytes` handles and written to the spill file.
Therefore, with the default policy, a 10 MiB upload does not incur the claimed
10 MiB second in-memory copy. The copy attributable to accumulation is bounded
to roughly the first 128 KiB. Capture itself is also conditional on a
body-aware request plugin or an explicit `RequestBodyBuffer` entry.

This is a confirmed performance cost with negligible default impact. A
`Vec<Bytes>` plus segment cursor is not accepted without profiling evidence:
it would complicate capture finalization, spill transition, immutable
snapshots, mutation, replay, and cancellation-safety. Most body-aware filters
materialize a contiguous body anyway, which would move rather than eliminate
the copy. Closed as an accepted tradeoff on 2026-08-11.

## Core Rationale

**1. The first HTTP/1 copy is required by the current ownership boundary.**

`BodyReader` owns and reuses a 64 KiB internal buffer. The server must return an
owned `Bytes` across async calls and to the application-supplied buffer, so
`Bytes::copy_from_slice` separates the chunk lifetime from that reusable read
buffer. Removing this copy would require a wider body-reader ownership redesign,
not the finding's proposed storage-only change.

**2. Pingora's reference buffer is not Edgion's production buffer.**

`InMemoryRequestBodyBuffer` is used as a reference implementation and by
Pingora tests. Edgion creates `BodyStorage` and registers it through
`set_request_body_buffer`; optimizing only the reference implementation would
not change Gateway request handling.

**3. Edgion bounds the second in-memory copy.**

`ResolvedRequestBodyConfig` defaults `memory_buffer_bytes` to 128 KiB and the
physical body cap to 32 MiB. `BodyStorage::write` appends only while the next
captured length remains under the memory threshold. Crossing the threshold
freezes the accumulated prefix and moves subsequent owned chunks to blocking
file writes using cheap `Bytes::clone()` handles. An operator may raise the
memory threshold, but that is an explicit memory/IO policy choice rather than
the default behavior.

**4. Segmentation does not universally remove materialization work.**

The request-filter trait defaults to requiring a materialized complete body.
For those consumers, a segmented initial representation must eventually be
coalesced into contiguous bytes. Capture-only and streaming-scan consumers may
benefit, but no profile currently shows the bounded append copy as a meaningful
Gateway CPU bottleneck.

## Fix Suggestions Not Accepted

- Change only Pingora's `InMemoryRequestBodyBuffer` to `Vec<Bytes>` — it is not
  the implementation registered by Edgion.
- Change Edgion `BodyStorage` to segmented capture solely to remove the bounded
  append copy — this expands correctness-sensitive spill, snapshot, mutation,
  replay, and cancellation state for an unmeasured benefit.
- Reserve from attacker-controlled `Content-Length` — this converts a bounded
  copy concern into memory-amplification risk and remains prohibited.

## Re-evaluation Triggers

Re-open only if:

- CPU profiling on realistic body-aware traffic attributes a material share of
  Gateway time to `BodyStorage::write` accumulation copies.
- The default memory spill threshold grows enough that the extra copy is no
  longer trivially bounded.
- A production consumer commonly performs capture/replay or streaming scans
  without ever materializing the body, and benchmarks show segmentation gives
  a meaningful end-to-end improvement.
- Pingora's HTTP/1 body reader changes to return independently owned chunks,
  materially changing the copy and lifetime calculus.

## Reference Cases

- Finding `pingora-review-16`, closed as an accepted tradeoff on 2026-08-11.
- Pingora source: `pingora-core/src/protocols/http/v1/server.rs`
  (`read_body_bytes`) and `pingora-core/src/protocols/http/body_buffer.rs`
  (`InMemoryRequestBodyBuffer`).
- Edgion source: `edgion-gateway/src/request_body/storage.rs` (`BodyStorage`),
  `edgion-gateway/src/request_body/config.rs` (resolved limits), and
  `edgion-gateway/src/plugins/runtime/session_adapter.rs` (registration and
  capture loop).
- Methodology: [performance-review-methodology.md](../performance-review-methodology.md).
