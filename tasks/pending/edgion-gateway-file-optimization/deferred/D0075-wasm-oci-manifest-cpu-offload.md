# D0075 — Offload Wasm OCI Manifest Parsing and Archive Extraction

## Meta
- Status: needs-design
- Severity: medium
- Discovered in: B0112 OCI manifest analysis
- Category: async-runtime
- Canonical task: [`tasks/todo/offload-wasm-oci-manifest-parsing-and-extraction.md`](../../../todo/offload-wasm-oci-manifest-parsing-and-extraction.md)

## Evidence
OCI manifest JSON parsing and compatibility gzip/tar extraction run synchronously inside async module-load paths. Network and decompression limits bound total work, but parsing a 4 MiB manifest or scanning and inflating up to 64 MiB can still occupy a Tokio worker during lazy load or background refresh.

## Why deferred
An ownership-preserving offload crosses `manifest.rs` and `oci/mod.rs`, and may require `vm_manager.rs` lifecycle coordination. It must move buffers without full-body clones while preserving permits, per-runtime-key single-flight, cancellation, negative caching, `Always` refresh, and last-known-good publication. B0112 instead hardens the parser and extractor within their current synchronous boundary.

## Integration-test impact
Use deterministic crate-local scheduling, ownership, cancellation, and parser/extractor tests plus existing OCI coverage. No existing integration-test edit or separate integration issue is needed.
