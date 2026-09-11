# B0128 — Gateway API Request Redirect and URL Rewrite

## Scope

- Baseline: `da2567094703a53ad928698be10388b450e39e43`
- Pipeline state: terminal; verified, awaiting local commit
- Active anchor crate: `edgion-gateway`
- Candidate owned files:
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/request_redirect.rs`
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/url_rewrite.rs`
- Baseline blobs: `901a361b015f2280dd38bc92c8632aa08ce67a86` and `88c661ed9d60160c4026b9ef96e45aa0f377751b`
- Exact implementation allow-list:
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/request_redirect.rs`
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/url_rewrite.rs`
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/mod.rs`
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/path_modifier.rs` (new)
  - `edgion-gateway/src/ctx.rs` (approved support only: remove unused SNI getter and its direct test assertion; retain TLS storage and JSON coverage)
- Forbidden files: every file outside an independently justified exact allow-list, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary

- Read both request-transform adapters, Gateway API schema validation, path/host/query handling, local-response and upstream routing lifecycles, tests, docs, and relevant review knowledge.
- Preserve URI encoding, redirect status/location safety, canonical path normalization, immutable publication, failure policy, access-log ownership, observability restraint, and response streaming.
- Existing integration tests remain frozen. Create a separate issue only if genuinely necessary new integration coverage cannot be represented without editing an existing file.

## Handoff

- Analysis: change both owned files. Both prefix transforms slice the current path by the configured match string's byte length, breaking root, trailing-slash, parameterized, and some join semantics. RequestRedirect also infers omitted scheme from SNI presence, so TLS without SNI can emit an `http` Location.
- Proposed scope: both owned adapters; `gateway_api_filters/mod.rs`; and a new private `gateway_api_filters/path_modifier.rs` shared by redirect and rewrite. Implement element-aware matched-prefix replacement with one boundary slash and derive omitted scheme from canonical `downstream_is_tls`, using HTTPS for unknown connection state.
- Complexity: complex and plan-first because two protocol-visible request paths gain one canonical transformation mechanism and the redirect origin security behavior changes.
- Preserved behavior: normalized current-path input, raw query preservation, Host/port rules, explicit scheme authority, local redirect EOS/termination, `set_upstream_uri`, failure/access-log policy, immutable publication, no tracing, and response streaming.
- Deferred findings: `D0092` owns shared validation/preparation parity across Kubernetes status, FS/etcd, EdgionPlugins, and the Gateway backstop. `D0093` owns paired EN/ZH RequestRedirect status-code and invalid backendRef example corrections. No integration issue is currently needed.
- Freshness: independently revalidated at `b71f37f2`. Both owned blobs remain identical to `da256709`; `path_modifier.rs` remains absent. B0127 changed `mod.rs` only by registering a private header helper with no path, URI, redirect, TLS, or session coupling, so the original value decision, complex classification, four-path scope, invariants, and test direction remain current.
- Implementation: `b0128_implementation` added the shared element-boundary helper and adapter regressions, borrowed match metadata, preserved raw query spelling, and replaced SNI-based scheme inference with `downstream_is_tls` (unknown defaults to HTTPS).
- Plan review: `b0128_plan_review` approved the revised plan after requiring explicit root/empty/trailing-slash outcomes and observable adapter assertions instead of construction smoke tests. It separately approved removing the now-unused `RequestInfo::sni()` getter; `ctx.rs` remains pending full coverage.
- Plan: (1) add the private helper with one result allocation; (2) register its module; (3) delegate redirect path replacement and canonical TLS inference; (4) delegate rewrite path replacement while preserving URI finalization/query; (5) run focused tests and full gates. The approved support cleanup removes six lines only.
- Review: `b0128_final_review` LGTM. No Must Fix or remaining Should Fix. Its two requested matrix cases now cover a configured trailing-slash prefix and a parameter whose multibyte character straddled the old byte offset.
- Verification: 10 focused tests passed (TLS facts 1, path helper 1, redirect 7, rewrite 1); the expanded helper matrix passed again. Root format, locked workspace check/Clippy, locked Gateway binary build, agent-doc/SSA/metrics/Pingora/no-tracing/English-only guards, diff whitespace, and frozen-path checks passed. Final check/Clippy refresh after the two test-only additions passed (11.26s/25.62s); Clippy retains existing warnings. Gateway binary build passed in 41.75s.
- Commit: `b6060c7a` (recovery commit shared with B0129/B0130 records); not pushed.

## Verified worktree fingerprints

These Git blob hashes identify the reviewed code before the recovery commit.

| File | Blob |
|------|------|
| `edgion-gateway/src/ctx.rs` | `67e10ee4f49b502f74646bc2d7b087fd825f2558` |
| `edgion-gateway/src/plugins/runtime/gateway_api_filters/mod.rs` | `a3829bea71d9ef955fb53e9b118621f181524173` |
| `edgion-gateway/src/plugins/runtime/gateway_api_filters/path_modifier.rs` | `1b5d5fd4998bd2eedd6acd2f0076e12b2f807aea` |
| `edgion-gateway/src/plugins/runtime/gateway_api_filters/request_redirect.rs` | `0760aef6d4333565bc37384ed631856a121a4c54` |
| `edgion-gateway/src/plugins/runtime/gateway_api_filters/url_rewrite.rs` | `293ccf12a7631613fcbc2c4a6239ff618550f473` |
