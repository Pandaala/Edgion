# B0111 — Wasm OCI Cache

## Scope
- Baseline: `874b5f14`
- Pipeline state: commit-ready
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
- Promotion freshness: `cache.rs` remains at the analyzed blob through `ee8fb4b5`. B0110 changed the parent OCI authentication/token transport path and tests, so an independent current-HEAD analyst must revalidate cache callers, ownership, and the one-file boundary before implementation.
- Refresh analysis: change, moderate, exact `cache.rs` only. Both purge and size eviction currently accept broad suffix/substring matches and can delete collision-shaped foreign files in the operator-selected cache directory. The exact writer-owned grammar is lowercase 64-hex plus `.blob`, `.manifest.json`, `.tmp.<canonical-positive-u32-pid>`, or `.manifest.tmp.<canonical-positive-u32-pid>`; uppercase, decorated/extra forms, invalid UTF-8, and noncanonical PIDs are foreign. One shared predicate must gate both scans without changing TTL, LRU, SHA, corrupt-entry, mtime, store/purge serialization, best-effort, or valid stale-temp semantics.
- Refresh freshness: external HEAD advanced to `874b5f14` through unrelated conf-sync/observability work while analysis ran. `cache.rs`, `oci/mod.rs`, `auth.rs`, `vm_manager.rs`, and `edgion-tests/**` remained unchanged and clean, so the refreshed call-chain result is current.
- Refresh tests: table-driven exact accepted/rejected names; purge preservation/removal; size-eviction exclusion of foreign collisions; and a Unix-only invalid-UTF-8 filename regression. Deterministic tempfile coverage is sufficient, so no integration issue is needed.
- Implementation: added one allocation-free `OsStr` classifier shared by both directory scans. It accepts only the exact lowercase digest/final/temp grammar and canonical positive `u32` PID, rejects collision-shaped, uppercase, malformed, overflow, wrong-temp, and invalid-UTF-8 names, and otherwise leaves cache behavior and observability unchanged.
- Implementation self-check: all 6 cache and 22 parent OCI tests passed; the targeted Gateway library check, format, diff, no-tracing, English-only, and frozen-path guards passed. On Apple filesystems the invalid-UTF-8 survival setup is rejected by the filesystem itself, so Unix directly tests raw-name rejection everywhere and runs filesystem survival on non-Apple Unix.
- Review: approved independently with no findings. The reviewer confirmed exact allocation-free writer grammar, rejection of every malformed/collision/non-UTF-8 form, shared use by purge and size eviction, preservation of valid stale-temp cleanup/accounting and all SHA/mtime/TTL/LRU/cap/locking/rename/best-effort/logging semantics, honest platform-aware tests, exact one-file scope, and no cache, observability, sensitive-data, integration, or D0074 leakage.
- Verification: all 41 focused OCI tests passed. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and frozen-path guards passed on `874b5f14`; Clippy retained only the existing warning set. Exact code scope is the one-file allow-list and no integration issue is needed.
- Commit: `93ccf75f`
