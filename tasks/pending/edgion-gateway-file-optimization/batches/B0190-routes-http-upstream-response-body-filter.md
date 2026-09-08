# B0190 — HTTP Upstream Response Body Filter

## Scope

- Analysis baseline: `1aad294d6933384413c943a51babb035dba679e2`
- Target blob: `092a451d6d10ed01aa02b4e8b3907dd7f52c9b15`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Target: `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_body_filter.rs`
- Existing `edgion-tests/**` files and protected user paths remain frozen.

## Handoff

- Analysis: change; complex. The read-only response-body dispatcher omits `grpc_route_unit`, `selected_backend`, and `selected_grpc_backend`, so accepted ExtensionRef BandwidthLimit configuration at those scopes silently does not run. Dispatch should mirror the response onion: gRPC rule else HTTP rule, selected gRPC backend else selected HTTP backend, Gateway, then global, retaining the maximum delay.
- Frozen technical scope: `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_body_filter.rs`, including inline tests only.
- Required preservation: mutable processors remain before read-only filters; mutually exclusive route/backend scope selection, Gateway/global order, streaming/RawWindow bounds, delay backpressure, terminal-before-trailers, EOS/trailer closure, SSE/AI/retry/access-log ownership remain unchanged.
- Baseline target tests: 24/24 passed. The existing route/backend dispatch-gap task already owns later integration coverage; no duplicate issue is needed.
- This is a queued candidate and cannot enter the serial mutation/review lane before B0189 commits.
