# B0190 — HTTP Upstream Response Body Filter

## Scope

- Analysis baseline: `1aad294d6933384413c943a51babb035dba679e2`
- Target blob: `092a451d6d10ed01aa02b4e8b3907dd7f52c9b15`
- Pipeline state: ready to commit
- Active anchor crate: `edgion-gateway`
- Target: `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_body_filter.rs`
- Existing `edgion-tests/**` files and protected user paths remain frozen.

## Handoff

- Analysis: change; complex. The read-only response-body dispatcher omits `grpc_route_unit`, `selected_backend`, and `selected_grpc_backend`, so accepted ExtensionRef BandwidthLimit configuration at those scopes silently does not run. Dispatch should mirror the response onion: gRPC rule else HTTP rule, selected gRPC backend else selected HTTP backend, Gateway, then global, retaining the maximum delay.
- Frozen technical scope: `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_body_filter.rs`, including inline tests only.
- Required preservation: mutable processors remain before read-only filters; mutually exclusive route/backend scope selection, Gateway/global order, streaming/RawWindow bounds, delay backpressure, terminal-before-trailers, EOS/trailer closure, SSE/AI/retry/access-log ownership remain unchanged.
- Baseline target tests: 24/24 passed. The existing route/backend dispatch-gap task already owns later integration coverage; no duplicate issue is needed.
- This is a queued candidate and cannot enter the serial mutation/review lane before B0189 commits.
- Previous batch: `12ec874a`; never push.
- Freshness: revalidated at `12ec874aa1475eb6f984e77ad19cbad3cc3e319a`; target blob remains `092a451d6d10ed01aa02b4e8b3907dd7f52c9b15`. B0189 changed request preparation only and does not affect this per-chunk response-body dispatch finding.

## Proposed Implementation Plan

1. Extend the allocation-free read-only dispatcher with mutually exclusive gRPC-or-HTTP route and selected-backend scopes.
2. Preserve Gateway then global dispatch and merge every returned delay by maximum.
3. Leave mutable processors, streaming windows, terminal/EOS/trailer arbitration, retry, AI, SSE, and access-log ownership untouched.
4. Add inline real-runtime coverage for each missing scope, gRPC/HTTP mutual exclusion, and the combined maximum delay.
5. Verify the target, full proxy-http module, repository gates, exact one-file technical scope, and frozen integrations.

Independent plan review approved the plan without a Must Fix. It confirmed the established response-stage onion, request-pinned runtimes, allocation-free maximum-delay composition, and isolation from mutable streaming and terminal lifecycle ownership.

Initial final review found one Must Fix: the first regression tests called only the merge helper and therefore did not prove the four context dispatch branches or gRPC-over-HTTP mutual exclusion. The batch returned to the same implementer for focused correction before approval.

## Outcome

- Implementation: read-only body plugins now run for exactly one request-pinned route runtime and one selected-backend runtime, preferring gRPC over HTTP at each layer, before the existing Gateway and global layers. Every returned delay is merged by maximum.
- Correction review: real context fixtures now exercise the gRPC and HTTP route/backend branches, both precedence pairs, and four-layer maximum-delay composition. Independent correction review returned LGTM with no remaining Must Fix or Should Fix.
- Verification: target 27/27 and full proxy-http 264/264; `cargo fmt --all`, locked workspace check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and frozen integration-path checks passed.
- Commit: pending; never push.
