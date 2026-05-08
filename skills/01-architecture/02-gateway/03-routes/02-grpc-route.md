---
name: gateway-grpc-route-engine
description: gRPC route engine — Service / Method matching, gRPC-Web support, HTTP pipeline integration.
---

# gRPC route engine

## Overview

gRPC routing is implemented based on the Gateway API `GRPCRoute` CRD. Service- and method-level routing is performed by parsing `/{service}/{method}` from the HTTP/2 path. The gRPC route is integrated into the HTTP pipeline and shares the Pingora ProxyHttp infrastructure.

## Module structure

```
src/core/gateway/routes/grpc/
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

## Key source files

| File | Responsibility |
|------|----------------|
| `src/core/gateway/routes/grpc/match_engine.rs` | GrpcMatchEngine, parse_grpc_path |
| `src/core/gateway/routes/grpc/match_unit.rs` | GrpcRouteRuleUnit, GrpcMatchInfo |
| `src/core/gateway/routes/grpc/routes_mgr.rs` | GlobalGrpcRouteManagers, GrpcRouteRules |
| `src/core/gateway/routes/grpc/conf_handler_impl.rs` | ConfHandler<GRPCRoute> implementation |
| `src/core/gateway/routes/grpc/integration.rs` | try_match_grpc_route, handle_grpc_upstream |
