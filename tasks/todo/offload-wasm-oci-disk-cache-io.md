# Offload Wasm OCI Disk Cache I/O

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-07 |
| Status | pending |
| Type | performance |
| Priority | P2 |
| Issue | N/A |
| Depends on | Coordinated `wasm/oci/cache.rs` and `wasm/oci/mod.rs` design |

## AI guide summary

Move bounded but potentially large OCI disk-cache reads, hashing, scans, eviction, and writes off Tokio worker threads without cloning full manifests or blobs or weakening module-load lifecycle guarantees.

## Requirements

- Inventory cache load, store, corruption cleanup, mtime touch, purge, eviction, and every async caller in `oci/mod.rs` and `vm_manager.rs`.
- Define an ownership-preserving blocking boundary that moves large buffers instead of cloning them.
- Preserve per-runtime-key single-flight, negative caching, immutable publication, background refresh, last-known-good retention, best-effort disk-cache failures, and process-local store serialization.
- Preserve the 4 MiB manifest, 64 MiB blob, and 64 KiB token exchange caps and the existing content-address verification contract.
- Define cancellation and shutdown behavior so abandoned async callers cannot leak permits, temporary files, or unbounded blocking work.
- Add no parallel cache implementation, new request-path logging, metrics, or tracing.
- Do not modify existing integration-test files.

## Acceptance criteria

- No potentially 64 MiB disk read, hash, scan, eviction, or write runs on a Tokio worker thread.
- The offload path performs no full-body clone merely to enter blocking work.
- Existing cache integrity, corruption cleanup, TTL, LRU, byte-cap, single-flight, negative-cache, refresh, and last-known-good semantics remain covered.
- Deterministic crate-local tests cover success, failure, cancellation, and temporary-file cleanup; no integration-test issue is required.
- Workspace format/check/Clippy, Gateway build, no-tracing, English-only, source-policy, and frozen-path guards pass.
