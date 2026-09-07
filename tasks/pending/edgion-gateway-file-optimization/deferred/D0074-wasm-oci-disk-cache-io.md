# D0074 — Offload Wasm OCI Disk Cache I/O

## Meta
- Status: needs-design
- Severity: medium
- Discovered in: B0111 OCI cache analysis
- Category: async-runtime
- Canonical task: [`tasks/todo/offload-wasm-oci-disk-cache-io.md`](../../../todo/offload-wasm-oci-disk-cache-io.md)

## Evidence
The OCI cache performs synchronous disk reads, SHA-256 hashing, directory scans and sorting, eviction, and writes from async module-load paths. A cache hit or store can read, hash, or write up to the 64 MiB blob cap and can therefore occupy a Tokio worker. Adjacent compilation, hashing, and purge work already establishes an offload boundary, but naively wrapping the cache functions in `spawn_blocking` would clone large buffers or blur ownership and cancellation.

## Why deferred
The correct change crosses `edgion-gateway/src/plugins/http/wasm/oci/cache.rs` and `oci/mod.rs`, including ownership transfer, single-flight behavior, error classification, shutdown, and last-known-good semantics. B0111 is limited to the destructive filename-ownership defect and must not add a partial scheduling wrapper.

## Integration-test impact
Use deterministic crate-local scheduling and ownership tests plus existing OCI cache coverage. No existing integration-test edit or separate integration issue is needed.
