# B0176 — HTTP Proxy Listener State

## Scope

- Candidate analysis HEAD: `efcc445f0ef4405d107d6937962969aff72c0d65`; freshness-revalidated and promoted at `6df7f67fd7787463bcedd163f6d1bed2ce243db0`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/routes/http/proxy_http/mod.rs`
- Exact technical implementation allow-list:
  - `edgion-gateway/src/routes/http/proxy_http/mod.rs`
  - `edgion-gateway/src/routes/http/proxy_http/pg_new_ctx.rs`
  - `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs`
  - `edgion-gateway/src/runtime/server/base.rs`
  - `edgion-gateway/src/runtime/server/listener_builder.rs`
  - `skills/01-architecture/02-gateway/03-routes/01-http-route.md`

## Handoff

- Analysis: `b0169_final_review` read the complete proxy facade and callbacks, sole listener construction chain, request initialization and HTTPS isolation, Pingora trait/dispatch/cache paths, RequestMirror implementation, tests, frozen integrations, and relevant knowledge; it refreshed the overlapping request-filter line after B0175.
- Decision: complex because the cohesive cleanup crosses Pingora lifecycle and request-hot state. Retain only the listener port and HTTPS fact actually used by requests, remove two proxy fields and the upstream ListenerContext GatewayClass clone chain with no consumers, and delete an `allow_spawning_subrequest` override that is observationally identical to Pingora's default false.
- Value: removes a full Listener clone/retention, unused strings/time state, a protocol comparison, and an impossible context lookup while shrinking immutable per-listener state.
- Preserved behavior: listener port, HTTPS-only 421 isolation, listener-key manager snapshots, hot reload, callback ordering, cache-internal subrequests, RequestMirror, response streaming, failure behavior, and all observability remain unchanged.
- Plan review: a fresh reviewer verified the sole construction sites, HTTP/HTTPS dispatcher equivalence, exact port cast, dead-field consumer searches, Pingora's default false application-subrequest hook, independent cache/RequestMirror behavior, scope, and stop conditions; verdict Approved with no Must Fix.
- Implementation: the proxy now retains only `listener_port` and `is_https_listener`; the upstream GatewayClass clone chain and unused time/class state are gone. Context creation and HTTPS isolation consume the derived facts, and deleting the unused override restores Pingora's identical default false.
- Final review: a fresh independent reviewer revalidated pinned Pingora dispatch, cache and RequestMirror boundaries, listener construction, 421 isolation, hot reload, callbacks, streaming, observability, documentation, scope, and frozen paths; verdict LGTM with no Must Fix or Should Fix.
- Testing: proxy tests passed 223/223, RequestMirror tests 25/25, and the complete HTTP route suite 345/345. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff, and 3,113-file frozen-path guards passed. Existing integrations remain frozen and sufficient.
- Deferred: stale route-manager cache wording is tracked in `tasks/todo/route-architecture-manager-cache-documentation.md`.
- Commit: pending; never push.
