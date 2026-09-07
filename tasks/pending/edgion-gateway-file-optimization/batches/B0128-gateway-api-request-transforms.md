# B0128 — Gateway API Request Redirect and URL Rewrite

## Scope

- Baseline: `da2567094703a53ad928698be10388b450e39e43`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Candidate owned files:
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/request_redirect.rs`
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/url_rewrite.rs`
- Baseline blobs: `901a361b015f2280dd38bc92c8632aa08ce67a86` and `88c661ed9d60160c4026b9ef96e45aa0f377751b`
- Candidate implementation allow-list: not frozen until analysis completes
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
- Deferred candidates: shared Gateway API request-transform validation parity across Kubernetes status, FS/etcd, EdgionPlugins, and Gateway backstop; paired EN/ZH RequestRedirect documentation correction for supported status codes and invalid backendRef examples. Reserve D0092–D0093 after B0127; no integration issue is currently needed.
- Freshness: analyzed at `da256709`; both owned blobs matched HEAD and the proposed shared path module is absent.
- Review: pending.
- Verification: pending.
- Commit: pending.
