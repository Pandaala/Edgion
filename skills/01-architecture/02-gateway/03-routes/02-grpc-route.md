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

`GrpcMatchEngine` supports both Exact and RegularExpression match types and implements a seven-level priority match:

| Priority | Match type | Data structure | Description |
|----------|------------|----------------|-------------|
| 1 | Exact service + Exact method | `HashMap<(String, String), Vec<...>>` | Exact match, O(1) |
| 2 | Exact service + Regex method | `Vec<Arc<GrpcRouteRuleUnit>>` | Exact service + regex method |
| 3 | Regex service + Exact method | `Vec<Arc<GrpcRouteRuleUnit>>` | Regex service + exact method |
| 4 | Regex service + Regex method | `Vec<Arc<GrpcRouteRuleUnit>>` | Both regex |
| 5 | Exact service only | `HashMap<String, Vec<...>>` | Service specified, matches any method |
| 6 | Regex service only | `Vec<Arc<GrpcRouteRuleUnit>>` | Regex service, no method specified |
| 7 | catch-all | `Option<Arc<GrpcRouteRuleUnit>>` | Neither service nor method specified |

gRPC path parsing (`parse_grpc_path`):

- Input: `/helloworld.Greeter/SayHello`
- Output: service = `helloworld.Greeter`, method = `SayHello`
- The path must have exactly two segments; otherwise an `InvalidGrpcPath` error is returned.

Multiple routes can share the same (service, method) combination (different hostnames or header conditions). They are stored as `Vec<Arc<GrpcRouteRuleUnit>>` and `deep_match` runs in order until the first match.

### Regex match

When `GRPCMethodMatch.type = RegularExpression`, the `type` applies to both the service and method fields (per Gateway API semantics). Regex patterns are precompiled into `Arc<Regex>` at route load time, stored in the `compiled_service_regex` / `compiled_method_regex` fields of `GrpcMatchInfo`. Regex routes are sorted by pattern length descending (longer = more specific = higher priority).

Unlike HTTPRoute's `RegexRoutesEngine`, gRPC regex matching does not use `RegexSet` — it walks the precompiled regexes linearly, since gRPC regex routes are usually few.

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
- gRPC does not bucket by hostname (unlike HTTP); all routes go into a single `GrpcRouteRules`.
- Inside `GrpcMatchEngine::match_route`, `deep_match` validates the request hostname against the route's `effective_hostnames`.
- Listener bucketing matches HTTP: routes with empty `resolved_listeners` are skipped + warned (no broadcast-to-all-ports fallback).

## Client deadline (`grpc-timeout`) handling

Edgion honors the client-supplied `grpc-timeout` header end-to-end:

1. `pg_request_filter::build_request_metadata` parses the header (format `{number}{unit}` with units `H | M | S | m | u | n`) via `grpc_timeout::parse_grpc_timeout`. The resulting deadline is stored as `ctx.request_info.grpc_client_deadline = start_time.checked_add(parsed_value)`.
2. `pg_upstream_peer::upstream_peer` short-circuits with `EdgionErrorCode::GrpcClientDeadlineExceeded` (raw control status 504 → downstream HTTP 200 + `grpc-status: 4`) when the deadline is already past at peer-selection time.
3. `pg_upstream_peer::configure_peer_timeouts` folds `grpc_remaining()` into the upstream `read/write_timeout` via `min(backend_timeout, route_request_remaining, grpc_remaining)`. The existing zero-duration guard prevents `Duration::ZERO` from reaching Pingora.
4. `pg_upstream_request_filter` rewrites the outgoing `grpc-timeout` header to the formatted remaining time so downstream gRPC servers see a consistent deadline.
5. Retry-decision sites (`pg_fail_to_connect`, `pg_error_while_proxy`, `pg_upstream_response_filter` status-code retry branch) treat `request_timeout_exceeded() || grpc_deadline_exceeded()` as the unified "deadline blown — do not retry" guard, so a tight client deadline blocks LB picks and circuit-breaker counter increments the caller has already given up on.
6. `pg_upstream_peer` retry backoff bounds the sleep by `min(request_remaining, grpc_remaining)` and re-checks both deadlines after the sleep; required so that a `retry.backoff` coarser than the client `grpc-timeout` cannot extend gateway work past the caller's deadline.

Coverage spans pure gRPC (`application/grpc`) and gRPC-Web (`application/grpc-web*`). Malformed headers fall back to the route-level `request_timeout` (no behavior change). See `skills/04-review/h2-grpc/timeout-honored.md` for the decision context.

## Key source files

| File | Responsibility |
|------|----------------|
| `edgion-gateway/src/routes/grpc/match_engine.rs` | GrpcMatchEngine, parse_grpc_path |
| `edgion-gateway/src/routes/grpc/match_unit.rs` | GrpcRouteRuleUnit, GrpcMatchInfo |
| `edgion-gateway/src/routes/grpc/routes_mgr.rs` | GlobalGrpcRouteManagers, GrpcRouteRules |
| `edgion-gateway/src/routes/grpc/conf_handler_impl.rs` | ConfHandler<GRPCRoute> implementation |
| `edgion-gateway/src/routes/grpc/integration.rs` | try_match_grpc_route, handle_grpc_upstream |
| `edgion-gateway/src/routes/grpc/grpc_timeout.rs` | parse_grpc_timeout, format_grpc_timeout |
