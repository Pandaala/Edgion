# B0077 — ForwardAuth Bounded Response Ownership and Static Diagnostics

## Scope
- Baseline: `77a25855`
- Pipeline state: committed and fully verified
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/forward_auth/{mod.rs,plugin.rs}`
- Exact implementation file: `edgion-gateway/src/plugins/http/forward_auth/plugin.rs`
- Expected no-change file: `edgion-gateway/src/plugins/http/forward_auth/mod.rs`
- Allowed tests: colocated tests in the implementation file only; replace current network-backed unit tests with mock-driven tests
- Forbidden files: every existing `edgion-tests/**` path, shared resource/config and docs, common outbound client/egress/TLS/DNS/template implementation, PluginRuntime/PluginStore, PluginSession/body state APIs, metrics/access-log implementation, Cargo manifests, other auth plugins, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact future commit allow-list: `edgion-gateway/src/plugins/http/forward_auth/plugin.rs`; `tasks/pending/edgion-gateway-file-optimization/03-subtasks.md`; `tasks/pending/edgion-gateway-file-optimization/07-changelog.md`; `tasks/pending/edgion-gateway-file-optimization/edgion-gateway-file-optimization.md`; `tasks/pending/edgion-gateway-file-optimization/batches/B0076-form-json-transform-owned-value-encoding.md`; and `tasks/pending/edgion-gateway-file-optimization/batches/B0077-forward-auth-bounded-response-ownership.md`

## Context and value gate
- Analysis subagent: `b0056_implementation`, revalidated against `77a25855`; target blobs are `8fcdf98dda2ee9af4765508a2ab47a59007a8845` for `mod.rs` and `1ab03884571fbf8622b89c297ef4d325295a8359` for `plugin.rs`.
- Baseline: 5 Gateway ForwardAuth, 13 resource-related, and 42 PluginRuntime tests pass.
- Transport failure currently inserts dynamic `OutboundCallError` text into PluginLog, where URLs, addresses, or dependency detail can escape the required static classification boundary.
- Denied auth responses copy the already bounded `Bytes` body into a new Vec/Bytes pair, potentially duplicating the full 1-MiB response cap.
- Configured client response headers are first cloned into a Vec of owned names/values before insertion, although the auth response HeaderMap can be borrowed directly in configured order.
- Request path and method are converted from borrowed session values to owned Strings per call, and the constant plugin name is allocated per generation.
- Four existing tests start wiremock servers, violating the repository unit-test boundary. A private owned-response decision seam can preserve behavior with MockPluginSession and no network.
- Decision: use static diagnostics, move bounded response bodies, borrow response headers and request metadata, return a static name, and isolate response decision from transport for deterministic unit tests.
- Expected no-change file: `mod.rs` remains the accurate minimal facade.

## Hard-rule boundary
- Preserve `ManagedOutboundClient` and all shared outbound client, TLS, egress, DNS, redirect, proxy, request-template, retry, health, rate-limit, EndpointSlice, and URL guard behavior.
- Preserve fixed auth fail-close, `statusOnError`, generic client JSON, typed `ForwardAuthServiceUnreachable` diagnostic, fail-close Dye order, auth delay, body predicate and BodyFail 403, success/status classification, and shutdown behavior.
- Preserve the response cap `min(conn.maxResponseBytes, 1 MiB)` and streaming enforcement. `forwardBody` continues using the single canonical request BodyStorage with no second cache.
- Preserve `clientHeaders` configuration order, first-value/replace semantics, invalid-header static log, Content-Type, `upstreamHeaders`, `hideCredentials`, hop-by-hop/framing filtering, and exact response bytes.
- Transport failures use a fixed `push_err("upstream unreachable")`; do not expose dependency detail through PluginLog or public bodies.
- Keep request-stage local response ownership unchanged; add no tracing, metrics, access-log fields, schema/failure-policy behavior, dependencies, integration-test edits, or new outbound/header mechanisms.

## Deferred candidates
- D0037 already owns canonical auth identity-header sanitization for ForwardAuth and every other emitting consumer. D0038 owns forged-header integration coverage and depends on D0037. Do not duplicate or partially implement either.
- D0024 owns request-template runtime unification. Other forwarding, framing, Content-Type, SSRF, and response-header issues have closed precedents and are not reopened.
- Existing frozen integration coverage has 25 cases across success/deny, forwarded metadata/body, delay, credentials, unreachable fail-close, Secret, required origin, hop-by-hop/Expect, predicates, and multiple endpoints. No new integration issue is required.

## Implementation
- Complexity: complex; sensitive logging, owned response transfer, ordered header propagation, request-body sequencing, and network-free test seam change together
- Implementer: `b0055_runtime_analysis`; plan-only phase complete and distinct from analyst and reviewers
- Plan reviewer: `b0076_final_code_review`; approved classification ownership, HeaderMap/body splitting, response write order, metadata borrow feasibility, pointer-identity tests, and network-free transport failure coverage
- Exact change: implemented the private owned-response decision seam, direct bounded Bytes transfer, borrowed response headers and path/method metadata, static name, fixed transport error classification, and network-free tests; the dynamic header name is cloned only at Pingora's required owned insertion boundary
- Test contract: network-free success propagation; deny status/body/Content-Type/client header; BodyFail 403; transport fail-close fixed log/diagnostic/Dye; ordinary deny without dependency-failure outcome; static identity

## Independent review
- Code reviewer: `b0077_final_code_review`; distinct from analyst, implementer, and plan reviewer
- Code verdict: approved with no must-fix findings; confirmed static diagnostics, body pointer transfer, classification and write order, header semantics, metadata borrowing, shared outbound isolation, and no-network test coverage
- Deferred adjudication: existing D0037/D0038/D0024 only; no new deferred or integration issue

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | protected external paths unchanged |
| Baseline focused tests | passed | 5 Gateway, 13 resource-related, and 42 PluginRuntime tests |
| Implementation and focused tests | passed | 6 Gateway, 13 resource-related, 34 PluginRuntime, and 69 shared outbound tests |
| Gateway all-target compile | passed | implementation and reviewer runs passed |
| Independent code review | passed | no must-fix findings; required dynamic header-name ownership clone accepted |
| Full campaign gates | passed | format, workspace check, Clippy, agent docs, SSA, metrics, Pingora source, and no-tracing |
| Final frozen integration paths | passed | immutable integration-test baseline unchanged |

## Handoff
- Ledger/changelog updated: yes; 1 changed and 1 examined-no-change row, all verification complete
- Commit: `5ad0d07f`
