# B0110 — Wasm OCI Authentication

## Scope
- Baseline: `f8e8541e`
- Pipeline state: commit-ready
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
- Current promotion freshness: the target and complete `wasm/oci` subtree remain unchanged through `f8e8541e`; B0108/B0109 and the intervening external commit did not touch this authentication boundary, so the independent analysis remains current.
- Analyst: independent read-only analysis completed at `3ca80316`; the target blob remained `efb60b7f83d458df10eda9171ccb195337f8ad39`.
- Decision: change. In one coherent authentication-boundary batch, reject HTTPS-registry to HTTP token-realm downgrade before sending credentials, parse Bearer scheme/parameters case-insensitively with bounded quoted-string and quoted-pair support, and make exact-registry/Docker-Hub credential precedence deterministic.
- Complexity: moderate. The changes are cold-path, locally bounded, and deterministic, with one test-support module file; they do not alter schema or runtime hot paths.
- Preserved behavior: schema-hidden/admin-redacted pull secrets, sensitive Authorization headers, canonical outbound client/TLS/egress/proxy/redirect policy, bounded bodies, retry/semaphore behavior, token error classification and session-local reuse, loopback operator-selected HTTP registries, cross-origin HTTPS realms, and current logs/metrics.
- Tests: colocated parser, malformed/duplicate parameter, credential-precedence, realm-policy, and zero-request downgrade regressions plus the focused OCI suite and targeted Gateway checks.
- Integration: deterministic crate-local coverage is sufficient; no integration issue or `edgion-tests/**` edit is needed.
- Deferred: none.
- Credential-precedence adjudication: an independent read-only adjudicator confirmed the total order as actual API host, then Docker Hub's canonical `docker.io`, then legacy `index.docker.io`; within a normalized host, a bare key precedes decorated scheme/path forms and equal decorated forms use bytewise lexical order. Selection is one pass with O(1) state, and only the winner is decoded; malformed higher-priority credentials fail closed without fallback.
- Implementation: added a single-pass Bearer challenge parser bounded to 16 KiB and 32 parameters with case-insensitive scheme/names, quoted-string/quoted-pair/comma handling, duplicate recognized-parameter rejection, and allocation-free unknown-parameter skipping. Credential selection now follows the adjudicated deterministic order. HTTPS registry sessions reject an HTTP token realm before constructing a credential-bearing request while explicit loopback HTTP registries and cross-origin HTTPS realms remain valid.
- Implementation self-check: all 39 focused OCI tests and the targeted Gateway all-target check passed; targeted Clippy exited zero with only the existing warning set; format, diff, Gateway no-tracing, English-only, and frozen-path guards passed. Exact code diff matches the two-file allow-list.
- Review: approved independently with no findings. The reviewer confirmed deterministic one-pass credential selection and winner-only fail-close decode; the parser's byte/parameter bounds, strict/case-insensitive grammar, quoting/escape/comma handling, duplicate rejection, and allocation-free unknown skipping; pre-request HTTPS-to-HTTP realm rejection; preserved loopback HTTP and HTTPS realm behavior; canonical client/egress/redirect/body/retry/token ownership; exact two-file scope; and no logging, metric, tracing, secret, integration, or failure-policy drift.
- Verification: all 39 focused OCI tests passed. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and frozen-path guards passed on `f8e8541e`; Clippy retained only the existing warning set. Exact code scope is the two-file allow-list and no integration issue is needed.
- Commit: pending
