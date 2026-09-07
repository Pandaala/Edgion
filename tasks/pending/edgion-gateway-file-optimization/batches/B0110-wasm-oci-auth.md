# B0110 — Wasm OCI Authentication

## Scope
- Baseline: `3ca80316`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/http/wasm/oci/auth.rs`
- Baseline blob: `efb60b7f83d458df10eda9171ccb195337f8ad39`
- Support file: `edgion-gateway/src/plugins/http/wasm/oci/mod.rs`
- Implementation allow-list: `edgion-gateway/src/plugins/http/wasm/oci/auth.rs`; `edgion-gateway/src/plugins/http/wasm/oci/mod.rs`
- Forbidden files: every file outside the exact two-file allow-list, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary
- Trace OCI registry authentication, credential/header handling, canonical outbound client and egress enforcement, token caching/lifecycle, error redaction, callers, sibling OCI operations, and deterministic coverage before deciding value.
- Preserve sensitive-data redaction, binary-owned operational logging, canonical access-log ownership, observability restraint, outbound HTTP/TLS/egress unification, bounded plugin-initiated exchanges, and no tracing.
- Existing integration tests remain a frozen validation floor. Prefer deterministic crate-local coverage; any genuinely necessary new integration coverage becomes a separate issue.

## Handoff
- Promotion freshness: the target and complete `wasm/oci` subtree were mechanically unchanged by external commits through `3ca80316`; the target blob remains `efb60b7f83d458df10eda9171ccb195337f8ad39`.
- Analyst: independent read-only analysis completed at `3ca80316`; the target blob remained `efb60b7f83d458df10eda9171ccb195337f8ad39`.
- Decision: change. In one coherent authentication-boundary batch, reject HTTPS-registry to HTTP token-realm downgrade before sending credentials, parse Bearer scheme/parameters case-insensitively with bounded quoted-string and quoted-pair support, and make exact-registry/Docker-Hub credential precedence deterministic.
- Complexity: moderate. The changes are cold-path, locally bounded, and deterministic, with one test-support module file; they do not alter schema or runtime hot paths.
- Preserved behavior: schema-hidden/admin-redacted pull secrets, sensitive Authorization headers, canonical outbound client/TLS/egress/proxy/redirect policy, bounded bodies, retry/semaphore behavior, token error classification and session-local reuse, loopback operator-selected HTTP registries, cross-origin HTTPS realms, and current logs/metrics.
- Tests: colocated parser, malformed/duplicate parameter, credential-precedence, realm-policy, and zero-request downgrade regressions plus the focused OCI suite and targeted Gateway checks.
- Integration: deterministic crate-local coverage is sufficient; no integration issue or `edgion-tests/**` edit is needed.
- Deferred: none.
- Review: pending
- Verification: pending
- Commit: pending
