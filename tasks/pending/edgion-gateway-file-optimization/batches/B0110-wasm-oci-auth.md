# B0110 — Wasm OCI Authentication

## Scope
- Baseline: `dde42825`
- Pipeline state: analysis-in-progress
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/http/wasm/oci/auth.rs`
- Baseline blob: `efb60b7f83d458df10eda9171ccb195337f8ad39`
- Implementation allow-list: pending analyst decision; no writes are authorized during analysis
- Forbidden files: all code during analysis, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary
- Trace OCI registry authentication, credential/header handling, canonical outbound client and egress enforcement, token caching/lifecycle, error redaction, callers, sibling OCI operations, and deterministic coverage before deciding value.
- Preserve sensitive-data redaction, binary-owned operational logging, canonical access-log ownership, observability restraint, outbound HTTP/TLS/egress unification, bounded plugin-initiated exchanges, and no tracing.
- Existing integration tests remain a frozen validation floor. Prefer deterministic crate-local coverage; any genuinely necessary new integration coverage becomes a separate issue.

## Handoff
- Analyst: pending
- Decision: pending
- Review: pending
- Verification: pending
- Commit: pending
