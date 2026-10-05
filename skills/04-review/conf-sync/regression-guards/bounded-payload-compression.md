---
name: bounded-payload-compression
description: List and Watch share threshold compression, bounded payloads and cancellation-aware decoding.
knowledge_type: regression-guards
---

# Bounded List and Watch payload compression

## Conclusion and scope

Controller and Gateway use `edgion-common::conf_sync::payload` for both List and
Watch. Payloads over 32 MiB use Zstd level 3 when smaller; identity preserves the
Bytes allocation. The old 64 MiB List rejection and implicit 4 MiB Gateway receive
limit are replaced by a 512 MiB decoded-payload budget and a message budget with
64 KiB metadata headroom. This is finite capacity, not unlimited List support.

The user selected threshold compression and unchanged snapshot semantics over
pagination. Resource schema, per-member WAF limits, mTLS and workqueue processing
are unchanged. There is no new acknowledgment or resource status condition:
ingest acceptance does not prove Gateway application. Transport oversize errors
are explicit `ResourceExhausted`; no rule text is truncated or partially published.

## Rationale and evidence

- The previous initial and recovery `ConfigSyncClientService::new` paths inherited
  Tonic 0.12.3's 4 MiB receive limit despite valid larger WAF resources and batches.
  `configured_client` now owns initial/recovery/TLS-reload receive configuration.
- ListResponse and WatchResponse carry byte data, encoding and uncompressed size.
  The shared decoder validates the encoding, exact length, incremental output and
  Zstd window; it rejects truncated frames, trailing data and concatenated frames.
- The codec permits at most two blocking jobs per process. Cancellation does not
  release a permit until its blocking job has finished. No codec IO or await is
  added to Controller resource processing.
- `ConfigSyncServer::list_grpc_cached` stores an encoded result in the exact raw
  cache entry's OnceCell. Raw `list_sync_cached` remains available to federation;
  thread-local admin redaction bypasses both caches. Identity adds no body copy.
  A completed encoding is shared; cancellation of the initializing request can
  leave the cell empty and a later caller may retry encoding that version.
- Gateway `decode_transport_payload` observes generation cancellation during and
  after decode; the result is discarded when cancellation is observed. This does
  not linearize the subsequent synchronous JSON publication with channel swaps.
  Malformed successful responses withdraw Backend state like invalid
  JSON, while generic resources retain their previous state and recover by List.
  Transport RPC errors retain the pre-existing handling.

Authority: root AGENTS.md, schema/transport and redaction rules, and
[Watch recovery](watch-decode-error-relist.md).

## Alternatives and limits

Compression reduces bandwidth; it does not remove the total configuration memory
cost. Raw List cache, compressed cache, protobuf buffers, decoding and runtime
objects can coexist. Serialization currently creates the raw JSON before checking
the codec budget. The budget is not a process RSS or peak-allocation guarantee.
Watch's four payload queues have capacity one, preserving backpressure with fewer
retained batches. Typed batches and the event ring remain count-bounded, not
byte-bounded. The previous small-event memory estimate is no longer applicable.

The [protocol overview](../../../01-architecture/03-controller-gateway-link/00-overview.md#operational-limits)
describes this same transport budget and scopes capacity one to each Watch
subscription's typed-event, JSON-conversion, server-forwarding and gRPC-forwarding
queues (`ServerCache::watch`/`watch_json`, `ConfigSyncServer::watch`, and
`ConfigSyncGrpcServer::watch`). The separate WatchServerMeta queue remains eight,
and the TLS connection handoff queue remains 64. `payload::encode` checks raw JSON
before compression; `payload::decode` bounds encoded bytes and declared decoded
size, and validates the exact decoded length. Compression cannot admit a raw
snapshot over the budget.

No pagination, resource deduplication, WAF-only bypass, or old-wire compatibility
shim is introduced. Controller/Gateway must be upgraded together for the explicit
payload envelope. Federation still uses its independent raw payload protocol.

## Verification and re-evaluation

Historical transport-fix verification (runtime tests not rerun for the documentation alignment):

- Common payload unit tests: 7 passed.
- Controller `--lib conf_sync` with `--no-default-features --features allocator-system`:
  85 passed; production gRPC `conf_sync_payload` integration test: 1 passed.
- Gateway `--lib conf_sync::cache_client`: 53 passed; `conf_sync_payload_client`
  (real mTLS initial/recovery/certificate reload) and `conf_sync_payload_dispatch`
  (production cache publication/recovery): 1 passed each.
- `cargo fmt --all`, `cargo check --workspace --all-targets`,
  `cargo clippy --workspace --all-targets`, agent-doc, SSA-force, metrics inventory,
  Gateway API transform contract and unit-test layout checks passed.
- Independent final code and knowledge review: LGTM.
- The Pingora guard initially failed because it expected `edgion_v5` while the
  manifest and lockfile already selected `edgion_0.9.0_v1`. The user subsequently
  authorized updating the guard and canonical policy to match that branch. The
  source selection and locked commit were not changed. The updated guard passed
  for all 16 fork packages, and six isolated policy cases verified acceptance of
  the current branch and a consistent release tag, and rejection of the old
  branch, mixed branches, mixed commits, and revision selectors. The complete
  quick-check suite and independent policy review subsequently passed, resolving
  the closure blocker. Full-workspace tests were not requested or run.

Regression tests cover the shared codec, raw/encoded snapshot cache reuse, actual
gRPC List/Watch delivery above the previous limits, client reconstruction, and
invalid/cancelled Gateway decoding. Consult the change's verification report for
commands actually run; this page does not claim a full end-to-end WAF traffic run.
Re-evaluate for snapshots exceeding the budget, measured transient-memory pressure,
slow receivers, protocol changes, or a requirement for independently upgraded peers.

The docs-only overview alignment passed independent read-only review, applied
formatting and the formatting check, workspace all-target check and Clippy,
agent-doc, SSA-force, metrics inventory, Pingora source policy, Gateway API
transform contract and unit-test layout checks. No runtime or integration tests
were added or rerun for this documentation correction.

## Resolution references

- `waf-conf-sync-message-size-contract`: cross-cutting transport defect exposed by
  WAF bundle sizes and repeated resolved bundles; user chose compression over paging.
- `align-conf-sync-operational-limits-overview` / `D0017`: docs-only correction of
  the overview's stale 64 MiB List cap and 100-entry forwarding claim against the
  current shared codec and all four Watch data queues. Runtime behavior, resource
  validation, metadata queues, and TLS limits are unchanged.
