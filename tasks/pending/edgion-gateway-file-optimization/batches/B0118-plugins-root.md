# B0118 — Plugins Root Facade

## Scope
- Baseline: `884069ac`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Candidate owned file: `edgion-gateway/src/plugins/mod.rs`
- Baseline blob: `369424a3465762a76f8eea12fb22b0c122714cd8`
- Candidate implementation allow-list: not frozen until analysis completes
- Forbidden files: every file outside the eventual exact allow-list, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary
- Read the root facade with HTTP/stream plugin module ownership, PluginRuntime construction and dispatch, public re-exports, binary callers, validation boundaries, tests, and review precedent.
- Preserve Controller/Gateway separation, canonical request/stream behavior, logging/access-log ownership, redaction, observability restraint, body/streaming invariants, and cross-cutting unification.
- Existing integration tests remain frozen. Create a separate issue only if genuinely necessary new integration coverage cannot be added without editing an existing file.

## Handoff
- Analysis: examined-no-change. The 17-line root is a compile-time namespace and selective convenience facade with no request-path work, allocation, locking, validation, logging, or runtime cost. Existing module visibility and re-exports have live internal or extension-facing ownership; pruning, symmetry expansion, visibility churn, and wording-only edits do not clear the value gate.
- Coverage: HTTP/gRPC plugin compilation/publication and request/response dispatch, stream/TCP/UDP/TLS enforcement, route/runtime consumers, binary linkage, nested facades, validation, unit tests, frozen integration coverage, and review precedent were traced at `884069ac`.
- Deferred observations: three `Instant::elapsed()` calls belong to the later `plugin_runtime.rs` batch. The architecture page's obsolete `PluginRuntime` clone claim is knowledge drift and must be independently validated before tracking outside this facade batch.
- Review: pending.
- Verification: pending.
- Commit: pending.
