---
name: gateway-grpc-route-engine
description: gRPC route engine — Service / Method matching, gRPC-Web support, HTTP pipeline integration.
---

# gRPC route engine

## Overview

gRPC routing is implemented based on the Gateway API `GRPCRoute` CRD. Service- and method-level routing is performed by parsing `/{service}/{method}` from the HTTP/2 path. The gRPC route is integrated into the HTTP pipeline and shares the Pingora ProxyHttp infrastructure.

## Module structure

```
edgion-gateway/src/routes/grpc/
├── mod.rs                # module entry and type exports
├── conf_handler_impl.rs  # ConfHandler<GRPCRoute> implementation
├── match_engine.rs       # GrpcMatchEngine: service/method route matching
├── match_unit.rs         # GrpcRouteRuleUnit: route match unit
├── routes_mgr.rs         # GrpcRouteRules / GlobalGrpcRouteManagers
└── integration.rs        # HTTP pipeline integration
```

## Service / Method matching

`GrpcMatchEngine` keeps one immutable ranked route array and indexes its numeric
ranks by the Route's declared hostnames through the shared Algo `RadixHostnameMatcher`. Lookup visits the
exact hostname, longest-to-shortest matching wildcard suffixes, then catch-all.
Only matching declared hostnames contribute precedence. Listener intersection
must not promote wildcard/hostless Route priority; `deep_match` independently
enforces the effective intersection. Predicate no-match continues within the
group and then to broader hostname groups; real errors propagate out of the
candidate loop. The outer gRPC dispatch keeps its existing error/no-match mapping.

Within each group, priority is descending service character count, method
character count, header count, then oldest route creation time, namespace/name,
rule index, and match index. Regex fields use pattern character counts.
Exact/regex type is not an additional priority tier.

| Candidate index | Request lookup |
|-----------------|----------------|
| Exact service + method | Nested borrowed HashMap lookup |
| Exact service only | The same service lookup, all-methods list |
| Exact method only | Borrowed method HashMap lookup |
| Regex | Precompiled service/method predicates |
| Any method | Outer method matcher omitted |

These lists contain ascending build-time ranks. A stack-only five-way merge
compares integer heads without allocating, sorting, locking, or comparing route
identities on the request path. Hostname fan-out stores ranks, not duplicate
rule/plugin/backend state. Regex candidates remain a linear list; no latency or
throughput equivalence is inferred from this design alone.

`parse_grpc_path` borrows service and method from `/{service}/{method}`.
Malformed paths return `InvalidGrpcPath`. Exact method-only matches any service;
a header-only match has no method constraint. Omitted type means Exact.

### Validation and regex compilation

Both planes validate the complete authored hostname list (0..16 names). Bare
`*`, malformed names and excessive lists withdraw the complete Route, including
status-less Gateway input; they never create catch-all or partial hostname groups.
Only omitted/empty lists are hostless. Named queries use already-normalized
authority input; non-DNS authority cannot hit a named group.

`GRPCRouteMatch::compile_method` is the shared Controller/Gateway policy.
`GrpcMatchInfo.compiled_method_match` stores Any, Exact, or precompiled optional
regex fields; None means invalid, never Exact/catch-all fallback. Any invalid
method predicate drops its whole rule before rule runtimes are built, including
status-less/stale Controller input. Valid sibling rules survive.

Explicit empty method matchers and empty/invalid Exact names are rejected.
For regex, an absent/empty field is unconstrained only when the other is
non-empty. Failure of either supplied pattern rejects the predicate. Compilation
is bounded and happens at configuration load time, never on the request path.
See [the regression guard](../../../04-review/routing/regression-guards/grpc-method-matching-and-precedence.md)
for authority, tests, and limitations.

## gRPC-Web support

The gRPC-Web protocol is identified during the `early_request_filter` phase via the request Content-Type:

- `application/grpc` → marked as `grpc`.
- `application/grpc-web` or `application/grpc-web+proto` → marked as `grpc-web`.

Content-Type matching is case-insensitive (RFC 9110 §8.3.1), so `Application/Grpc` and `APPLICATION/GRPC-WEB+PROTO` match too. The shared helpers live in `proxy_http::content_type`.

The result is stored in `ctx.request_info.discover_protocol`. The `is_grpc_protocol` helper checks this field; gRPC and gRPC-Web share the same route-matching logic.

## HTTP pipeline integration

The gRPC route is integrated into the HTTP request handling pipeline via the `integration.rs` module:

### try_match_grpc_route

Called inside `pg_request_filter`, attempts to match a gRPC route:

1. Parse the gRPC path into service / method and store in `ctx.request_info`.
2. Call `DomainGrpcRouteRules::match_route` to perform matching.
3. On success: set `ctx.grpc_route_unit` and `ctx.gateway_info`, mark `ctx.is_grpc_route_matched = true`.
4. On failure: return `Ok(false)` and fall back to HTTP route matching.

### handle_grpc_upstream

Called inside `upstream_peer`, handles gRPC backend selection:

1. Check whether a backend is already selected (`ctx.selected_grpc_backend`).
2. From the rule of `grpc_route_unit`, weighted-select a backend via `GrpcRouteRules::select_backend`.
3. Establish the upstream connection (sharing the backend connection logic with HTTP routes).

### Backend selection

`GrpcRouteRules::select_backend` uses `BackendSelector` to perform weighted random selection:

- Default weight is 1 (when not specified).
- Supports `ref_denied` checks (cross-namespace references require a ReferenceGrant).

## Route management

`GlobalGrpcRouteManagers` follows the same per-listener management pattern as HTTP:

```
GlobalGrpcRouteManagers
├── route_cache: DashMap<String, GRPCRoute>
├── by_listener: DashMap<ListenerKey, Arc<GrpcPortRouteManager>>
└── route_units_cache: Mutex<HashMap<String, Vec<Arc<GrpcRouteRuleUnit>>>>
```

Specifics:

- `route_units_cache` caches parsed route units to avoid re-parsing on every listener rebuild.
- One per-listener `GrpcRouteRules` owns the ranked engine and its hostname indexes.
- A rank-changing rebuild replaces the rank array and all five-stream candidate
  contents together; old numeric ranks never enter a new array. The hostname
  tree is reused whenever final keys agree, including full-list and Gateway
  rebuilds. Unchanged cached units keep their runtime/selector Arcs. Discovery
  still scans caches and affected Listener payloads; this is not per-host ranking.
- Inside `GrpcMatchEngine::match_route`, `deep_match` validates the request hostname against the route's `effective_hostnames`.
- Listener bucketing matches HTTP: routes with empty `resolved_listeners` are skipped + warned (no broadcast-to-all-ports fallback).

## Client deadline (`grpc-timeout`) handling

Edgion propagates and applies the client-supplied `grpc-timeout` budget:

1. `pg_request_filter::build_request_metadata` parses the header (format `{number}{unit}` with units `H | M | S | m | u | n`) via `grpc_timeout::parse_grpc_timeout`. A successful `start_time.checked_add(parsed_value)` is stored in `RequestGrpcInfo.client_deadline` through `RequestInfo::set_grpc_client_deadline`.
2. `pg_upstream_peer::upstream_peer` short-circuits with `EdgionErrorCode::GrpcClientDeadlineExceeded` (raw control status 504 → downstream HTTP 200 + `grpc-status: 4`) when the deadline is already past at peer-selection time.
3. `pg_upstream_peer::configure_peer_timeouts` folds the remaining budget into upstream read/write timeouts with the backend request and route request bounds. A separate connection fold uses configured connect timeout, route request remaining and gRPC remaining; it sets phase and total TCP/TLS establishment bounds and records `connect_timeout_source`. Read/write retains its zero-duration guard; the connection fold preserves expired zero budgets.
4. `pg_upstream_request_filter` rewrites the outgoing `grpc-timeout` header to the formatted remaining time so downstream gRPC servers see a consistent deadline.
5. Retry-decision sites (`pg_fail_to_connect`, `pg_error_while_proxy`, `pg_upstream_response_filter` status-code retry branch) treat `request_timeout_exceeded() || grpc_deadline_exceeded()` as the unified "deadline blown — do not retry" guard, so a tight client deadline blocks LB picks and circuit-breaker counter increments the caller has already given up on.
6. `pg_upstream_peer` retry backoff bounds the sleep by `min(request_remaining, grpc_remaining)` and re-checks both deadlines after the sleep; required so that a `retry.backoff` coarser than the client `grpc-timeout` cannot extend gateway work past the caller's deadline.

Coverage spans pure gRPC (`application/grpc`) and gRPC-Web (`application/grpc-web*`). Malformed headers fall back to the route-level `request_timeout` (no behavior change). See `skills/04-review/h2-grpc/regression-guards/timeout-honored.md` for the decision context.

The total connection timer covers L4 and TLS, not offload dispatch, connection
pool acquisition or subsequent HTTP/2 session setup. Read/write timeouts remain
per-operation, so this is not one absolute timer across a streaming RPC.

## Key source files

| File | Responsibility |
|------|----------------|
| `edgion-gateway/src/routes/grpc/match_engine.rs` | GrpcMatchEngine, parse_grpc_path |
| `edgion-gateway/src/routes/grpc/match_unit.rs` | GrpcRouteRuleUnit, GrpcMatchInfo |
| `edgion-gateway/src/routes/grpc/routes_mgr.rs` | GlobalGrpcRouteManagers, GrpcRouteRules |
| `edgion-gateway/src/routes/grpc/conf_handler_impl.rs` | ConfHandler<GRPCRoute> implementation |
| `edgion-gateway/src/routes/grpc/integration.rs` | try_match_grpc_route, handle_grpc_upstream |
| `edgion-gateway/src/routes/grpc/grpc_timeout.rs` | parse_grpc_timeout, format_grpc_timeout |
