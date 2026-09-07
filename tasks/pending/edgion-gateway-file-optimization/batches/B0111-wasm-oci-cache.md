# B0111 — Wasm OCI Cache

## Scope
- Baseline: `16613ca6`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/http/wasm/oci/cache.rs`
- Baseline blob: `61b12c2f56bd3e4a3aab6d2b66d3facaea87ac09`
- Implementation allow-list: `edgion-gateway/src/plugins/http/wasm/oci/cache.rs`, including inline tests
- Forbidden files: every file outside the owned file, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary
- Trace OCI module cache keys, immutable value ownership, positive/negative lifetime, concurrent fetch coalescing, refresh and last-known-good behavior, capacity/eviction bounds, callers, and deterministic coverage before deciding value.
- Preserve the canonical cache-selection rules, bounded memory, sensitive-data redaction, binary-owned logging, canonical access-log ownership, observability restraint, outbound unification, and no tracing.
- Existing integration tests remain a frozen validation floor. Prefer deterministic crate-local coverage; any genuinely necessary new integration coverage becomes a separate issue.

## Handoff
- Analyst: independent read-only analysis completed at `16613ca6`; the target blob remained `61b12c2f56bd3e4a3aab6d2b66d3facaea87ac09`.
- Decision: change. Replace broad suffix/substring ownership checks with an exact ASCII classifier for 64-hex final entries and the two generated nonempty-decimal-PID temporary forms, so purge and eviction cannot delete collision-shaped foreign files.
- Complexity: moderate. The correction is deterministic and contained in one cold-path cache file.
- Preserved behavior: SHA verification, corrupt-entry cleanup, mtime touch, strict `age > ttl`, LRU and size-cap ordering, best-effort failures, store/purge serialization, valid stale-temp cleanup, no new logs/metrics/tracing, and no sensitive output.
- Tests: extend tempfile purge coverage and add table-driven exact accepted/rejected filename cases, including foreign suffix/substring collisions and non-UTF-8 names.
- Integration: crate-local filesystem tests are sufficient; no integration issue or `edgion-tests/**` edit is needed.
- Deferred: D0074 owns the cross-file, zero-copy async/blocking boundary for potentially 64 MiB cache I/O.
- Review: pending
- Verification: pending
- Commit: pending
