# Offload Wasm OCI Manifest Parsing and Archive Extraction

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-07 |
| Status | pending |
| Type | performance |
| Priority | P2 |
| Issue | N/A |
| Depends on | Coordinated `wasm/oci/manifest.rs` and `wasm/oci/mod.rs` design |

## AI guide summary

Move bounded but potentially CPU-heavy OCI manifest JSON parsing and compatibility gzip/tar extraction off Tokio worker threads without cloning full manifests, layers, or extracted modules.

## Requirements

- Inventory manifest/index parsing, platform and layer selection, archive extraction, and every async caller in `oci/mod.rs` and `vm_manager.rs`.
- Define an ownership-preserving blocking boundary that moves existing buffers instead of cloning them.
- Preserve request permits, retry/error classification, per-runtime-key single-flight, negative caching, cancellation, background `Always` refresh, and last-known-good publication.
- Preserve B0112 parser/extractor limits, the 4 MiB manifest and 64 MiB blob/extraction caps, digest verification, cache ownership, and generic redacted errors.
- Coordinate with D0074 without combining CPU parsing/extraction and disk-cache I/O into an unbounded blocking job.
- Add no request-path logging, metrics, tracing, or alternate parser/cache implementation.
- Do not modify existing integration-test files.

## Acceptance criteria

- Manifest parsing and gzip/tar scanning no longer execute on Tokio worker threads.
- Entering blocking work performs no full-body clone.
- Cancellation, permit release, single-flight, negative-cache, refresh, and last-known-good behavior remain deterministic.
- Crate-local tests cover success, bounded failure, cancellation, and task cleanup; no integration-test issue is required.
- Workspace format/check/Clippy, Gateway build, no-tracing, English-only, source-policy, and frozen-path guards pass.
