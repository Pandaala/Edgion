# Route Matching Architecture

> Deep reference for HTTP/gRPC route matching in Edgion Gateway.
> Covers the full data flow from config registration to per-request matching,
> including the per-port route table design, match engines, and listener-level
> multi-Gateway support.

## Overview

The route matching system resolves incoming HTTP/gRPC requests to backend
services through a multi-level matching pipeline with **per-port route
isolation** (aligned with Gateway API's listener-level scoping):

```
Request (port, hostname, path, headers, query, method)
  → Listener (physical TCP binding, one per port)
    → Per-port route table lookup:
      → Domain match (exact → wildcard → catch-all)
        → Path match (regex → radix tree: exact → prefix)
          → Deep match (method, headers, query params, Gateway/Listener constraints)
            → RouteMatchResult { route_unit, matched_gateway }
```

Each listener port has its own route table (`DomainRouteRules`), matching
the pattern used by TCP/UDP/TLS routes. Routes are assigned to ports via
`resolved_ports` (set by the controller during `parentRef` resolution) or
via fallback logic at the gateway side.

## Key Data Structures

### GlobalHttpRouteManagers (per-port route management)

```
GlobalHttpRouteManagers
├── route_cache: DashMap<String, HTTPRoute>
│   Canonical store of all HTTPRoute resources (keyed by "namespace/name").
│
└── by_port: DashMap<u16, Arc<HttpPortRouteManager>>
    Per-port managers. Each holds an independent route table.
```

```
HttpPortRouteManager
└── route_table: ArcSwap<DomainRouteRules>
    Lock-free route table for one listener port.
    Updated atomically via ArcSwap::store().
```

Routes are assigned to ports using a three-tiered strategy in
`bucket_routes_by_port()`:
1. `route.spec.resolved_ports` — controller-resolved listener ports (primary)
2. `parentRef.port` — direct port from parentRef (fallback #1)
3. All known ports in `by_port` — backward-compatible catch-all (fallback #2)

Gateway handler triggers `rebuild_all_port_managers()` after
`PortGatewayInfoStore` is updated, ensuring routes cached before
Gateways arrived are correctly distributed.

**Key file:** `src/core/gateway/routes/http/routes_mgr.rs`

### DomainRouteRules (global domain matching)

```
DomainRouteRules
├── exact_domain_map: ArcSwap<HashMap<String, Arc<RouteRules>>>
│   O(1) lookup for exact hostnames (e.g., "api.example.com")
│   Lowercase keys for case-insensitive matching
│
└── wildcard_engine: ArcSwap<Option<RadixHostMatchEngine<RouteRules>>>
    O(log n) lookup for wildcard hostnames (e.g., "*.example.com")
```

**Matching priority (per Gateway API spec):**
1. Exact domain match (HashMap O(1))
2. Wildcard domain match (RadixHostMatchEngine O(log n))
3. Catch-all `"*"` (from HTTPRoutes with no `spec.hostnames`)

### RouteRules (per-hostname path matching)

```
RouteRules
├── match_engine: Option<Arc<RadixRouteMatchEngine>>
│   Radix tree for Exact and PathPrefix routes
│
├── regex_routes_engine: Option<Arc<RegexRoutesEngine>>
│   RegexSet for RegularExpression path routes
│
├── route_rules_list: RwLock<Vec<Arc<HttpRouteRuleUnit>>>
│   All non-regex routes (for introspection/debugging)
│
├── regex_routes: RwLock<Vec<Arc<HttpRouteRuleUnit>>>
│   All regex routes (for introspection/debugging)
│
└── resource_keys: RwLock<HashSet<String>>
    Which HTTPRoute resources contribute to this hostname
```

**Path matching priority:**
1. Regex routes (checked first via RegexSet)
2. Radix routes: exact match (FullyConsumed) > prefix match (SegmentBoundary)
3. Within same category: longer path > shorter path
4. Within same length: more header matchers > fewer header matchers

### HttpRouteRuleUnit (single matchable rule)

```
HttpRouteRuleUnit
├── resource_key: String          "namespace/name" of the HTTPRoute
├── matched_info: MatchInfo       namespace, name, rule_id, match_id, match_item
├── rule: Arc<HTTPRouteRule>      Contains backend_refs, filters, plugin_runtime
├── path_regex: Option<Regex>     Compiled regex (only for RegularExpression type)
└── parent_refs: Option<Vec<ParentReference>>
    Used by deep_match for Gateway/Listener constraint checking
```

### RouteMatchResult (match output)

```
RouteMatchResult
├── route_unit: Arc<HttpRouteRuleUnit>   The matched route
└── matched_gateway: GatewayInfo         The specific gateway that satisfied parentRef constraints
```

**Key file:** `src/core/gateway/routes/http/match_unit.rs`

## Match Engines

### RadixRouteMatchEngine

Uses a radix tree for efficient path matching with single-pass traversal.

```
Build phase (at config time):
  1. Extract paths from all HttpRouteRuleUnit
  2. Normalize paths (consecutive slashes, trailing slash)
  3. Insert normalized paths into RadixTreeBuilder
  4. Freeze into immutable RadixTree

Match phase (per request):
  1. tree.match_all_ext(path) → Vec<(tree_value, MatchKind)>
  2. Filter by MatchKind:
     - Exact routes: require FullyConsumed
     - Prefix routes: accept FullyConsumed or SegmentBoundary, reject PartialSegment
  3. Sort candidates by priority_weight (desc), then header_matcher_count (desc)
  4. Run deep_match on each candidate until one succeeds
```

**Key file:** `src/core/gateway/routes/http/match_engine/radix_route_match.rs`

### RegexRoutesEngine

Uses `regex::RegexSet` for batch regex matching.

```
Build phase:
  1. Collect all regex patterns
  2. Try to build RegexSet (one-pass matching of all regexes)
  3. Sort routes by pattern length (longest first)

Match phase:
  1. regex_set.matches(path) → matching indices
  2. For each match (longest first): run deep_match
  3. First deep_match success wins
```

**Key file:** `src/core/gateway/routes/http/match_engine/regex_routes_engine.rs`

### RadixPath (route path metadata)

```
RadixPath
├── original: String        Original path from HTTPRoute
├── normalized: String      Cleaned path (no consecutive/trailing slashes)
├── route_idx: usize        Index into routes vec
├── is_prefix_match: bool   PathPrefix vs Exact
├── has_params: bool        Contains path parameters (e.g., {id})
├── segment_count: usize    Number of path segments
└── priority_weight: u32    Computed priority for sorting
```

Priority weight formula: `base(exact=2000, prefix=1000) + segment_count * 10 + (param ? 0 : 5)`

**Key file:** `src/core/gateway/routes/http/match_engine/radix_path.rs`

## Deep Match (Gateway/Listener Constraint Checking)

After path matching finds candidates, `deep_match` validates:

```
deep_match_common(matched_info, req_header, parent_refs, ctx, gateway_infos)
  │
  ├─ 0. check_gateway_listener_match(parent_refs, gateway_infos, ...)
  │     Iterates (parentRef × gatewayInfo) pairs:
  │     ├─ ParentRef matches gateway (namespace + name)?
  │     ├─ sectionName matches listener (if specified)?
  │     ├─ Request hostname matches listener hostname constraint?
  │     └─ AllowedRoutes permits this route kind from this namespace?
  │     → Returns Some(GatewayInfo) on first match, None otherwise
  │
  ├─ 1. HTTP Method match (if specified)
  │
  ├─ 2. Header matches (AND logic, all must match)
  │     Supports: Exact, RegularExpression
  │
  └─ 3. Query parameter matches (AND logic, all must match)
        Supports: Exact, RegularExpression
```

**Key file:** `src/core/gateway/runtime/matching/route.rs`

### GatewayInfo

Pre-built per Gateway/Listener at startup, collected per port, carried
through EdgionHttp to request processing:

```
GatewayInfo
├── namespace: Option<String>     Gateway namespace
├── name: String                  Gateway name
├── listener_name: Option<String> Listener's name
├── metrics_test_key: Option<String>
└── metrics_test_type: Option<TestType>
```

**Key file:** `src/core/gateway/runtime/store/config.rs`

## Multi-Gateway Port Sharing

### Architecture

Multiple Kubernetes Gateway resources can declare the same port. Pingora
binds one physical listener per port. All Gateways on the same port share
a single `EdgionHttp` instance. Gateway info is fetched dynamically from
`PortGatewayInfoStore` at request time, so Gateways added/removed at
runtime are reflected without restart.

```
runtime/server/base.rs:
  1. Pre-collect GatewayInfo per port:
     port_gateway_infos: HashMap<u16, Vec<GatewayInfo>>

  2. For each unique port, create one ListenerContext:
     ListenerContext { listener, ... }

  3. runtime/server/listener_builder creates EdgionHttp:
     EdgionHttp { listener, ... }
     (gateway_infos fetched dynamically per request from PortGatewayInfoStore)
```

### Request Flow

```
pg_request_filter():
  port = edgion_http.listener.port
  gateway_infos = get_port_gateway_info_store().get(port)

  1. Try gRPC match (if applicable):
     grpc_port_manager = get_global_grpc_route_managers().get_or_create_port_manager(port)
     grpc_routes = grpc_port_manager.load_route_table()
     try_match_grpc_route(&grpc_routes, session, ctx, &gateway_infos)

  2. Try HTTP match (if gRPC not matched):
     http_port_manager = get_global_http_route_managers().get_or_create_port_manager(port)
     port_routes = http_port_manager.load_route_table()
     port_routes.match_route(session, ctx, &gateway_infos)
     → Returns RouteMatchResult { route_unit, matched_gateway }

  3. No fallback needed — per-port lookup handles route isolation
```

**Key file:** `src/core/gateway/routes/http/proxy_http/pg_request_filter.rs`

## Route Registration (Config Time)

### HTTPRoute → GlobalHttpRouteManagers flow

```
ConfigClient receives HTTPRoute
  → ConfHandler<HTTPRoute>.full_set() or .partial_update()
    → GlobalHttpRouteManagers stores HTTPRoute in route_cache (DashMap)
    → rebuild_all_port_managers():
        1. Pre-create port managers for ports from parentRef.port + PortGatewayInfoStore
        2. bucket_routes_by_port():
           resolved_ports → parentRef.port → all known ports (fallback chain)
        3. For each port bucket:
           parse_http_routes_to_domain_rules():
             for each HTTPRoute in this port's bucket:
               validate(route) → parent_refs, rules, namespace, name
               effective_hostnames = resolve_all_effective_hostnames()
               for each hostname:
                 for each rule + match:
                   create HttpRouteRuleUnit (or regex variant)
                   add to domain_rules_map[hostname]
           Build DomainRouteRules (RadixRouteMatchEngine + RegexRoutesEngine)
           Atomically replace port's route_table via ArcSwap::store()

Gateway ConfHandler also triggers rebuild_all_port_managers() after
PortGatewayInfoStore is updated, to ensure routes that arrived before
Gateways are re-distributed to the correct ports.
```

### Effective Hostname Resolution

Per Gateway API spec, if HTTPRoute has no `spec.hostnames`:
1. Inherit hostname from the listener specified by `parentRef.sectionName`
2. If no sectionName, use first listener's hostname
3. If listener has no hostname, fall back to catch-all `"*"`

**Key function:** `resolve_all_effective_hostnames()` in `src/core/gateway/routes/http/conf_handler_impl.rs`

### Partial Update Strategy

- **Port scoping**: `partial_update` identifies affected ports (from
  `resolved_ports` / `parentRef.port` / all known ports), then rebuilds only
  those ports' route tables via `rebuild_affected_port_managers()`.
- **Exact domains**: Fine-grained RCU (Read-Copy-Update) — clone HashMap, update
  affected hostnames, atomically swap via inner ArcSwap
- **Wildcard domains**: Rebuild RadixHostMatchEngine, but reuse `Arc<RouteRules>`
  for unaffected hostnames (Arc reuse optimization)
- **Atomicity**: Each port's `HttpPortRouteManager` stores a complete
  `DomainRouteRules` via `ArcSwap::store()`. Readers see a consistent
  snapshot per port.

## gRPC Route Matching

gRPC routes follow the same per-port isolation pattern as HTTP:

```
GlobalGrpcRouteManagers
├── route_cache: DashMap<String, GRPCRoute>
├── by_port: DashMap<u16, Arc<GrpcPortRouteManager>>
└── route_units_cache: Mutex<HashMap<String, Vec<Arc<GrpcRouteRuleUnit>>>>

GrpcPortRouteManager
└── route_table: ArcSwap<DomainGrpcRouteRules>

DomainGrpcRouteRules
└── grpc_routes: ArcSwap<GrpcRouteRules>
    └── match_engine: Option<Arc<GrpcMatchEngine>>
        ├── exact_routes: HashMap<(service, method), Vec<Arc<GrpcRouteRuleUnit>>>
        ├── service_routes: HashMap<service, Vec<Arc<GrpcRouteRuleUnit>>>
        └── catch_all_route: Option<Arc<GrpcRouteRuleUnit>>
```

**gRPC match priority:**
1. Exact (service + method)
2. Service-level (service only)
3. Catch-all

Each candidate runs `deep_match` with `gateway_infos` for Gateway/Listener
validation, identical to HTTP routes. Port resolution uses the same
fallback chain as HTTP (resolved_ports → parentRef.port → all known ports).

**Key files:** `src/core/gateway/routes/grpc/`

## Request Lifecycle Integration

```
Pingora receives request
  │
  ├─ new_ctx() → EdgionHttpContext { gateway_info: first from gateway_infos }
  │
  ├─ early_request_filter() → ACME challenges, timeouts, keepalive
  │
  ├─ request_filter()
  │   ├─ build_request_metadata() → hostname, path, protocol, client_addr
  │   ├─ gateway_infos = PortGatewayInfoStore.get(port)
  │   ├─ HTTPS isolation check (SNI/Host mismatch → 421)
  │   │
  │   ├─ gRPC route match (if applicable)
  │   │   └─ grpc_port_mgr = get_global_grpc_route_managers().get_or_create_port_manager(port)
  │   │       grpc_routes = grpc_port_mgr.load_route_table()
  │   │       try_match_grpc_route(&grpc_routes, session, ctx, &gateway_infos)
  │   │       → on match: ctx.gateway_info = matched GatewayInfo
  │   │
  │   ├─ HTTP route match (if gRPC not matched)
  │   │   └─ http_port_mgr = get_global_http_route_managers().get_or_create_port_manager(port)
  │   │       port_routes = http_port_mgr.load_route_table()
  │   │       port_routes.match_route(session, ctx, &gateway_infos)
  │   │       → on match: ctx.gateway_info = result.matched_gateway
  │   │                    ctx.route_unit = result.route_unit
  │   │
  │   ├─ Preflight handling (CORS)
  │   ├─ Global plugins (from EdgionGatewayConfig)
  │   └─ Route-level plugins (from matched route's plugin_runtime)
  │
  ├─ upstream_peer() → backend selection via RouteRules::select_backend()
  │
  ├─ upstream_response_filter() → response plugins
  │
  └─ logging() → AccessLogEntry built from EdgionHttpContext
```

---

## Key Files Reference

| File | Purpose |
|------|---------|
| `src/core/gateway/routes/http/routes_mgr.rs` | GlobalHttpRouteManagers, HttpPortRouteManager, DomainRouteRules, RouteRules |
| `src/core/gateway/routes/http/match_unit.rs` | HttpRouteRuleUnit, RouteMatchResult, deep_match |
| `src/core/gateway/routes/http/conf_handler_impl.rs` | HTTPRoute → per-port route registration |
| `src/core/gateway/routes/http/match_engine/radix_route_match.rs` | RadixRouteMatchEngine |
| `src/core/gateway/routes/http/match_engine/radix_path.rs` | RadixPath compilation |
| `src/core/gateway/routes/http/match_engine/regex_routes_engine.rs` | RegexRoutesEngine |
| `src/core/gateway/routes/http/proxy_http/pg_request_filter.rs` | Request filter + per-port route matching |
| `src/core/gateway/routes/http/proxy_http/mod.rs` | EdgionHttpProxy struct |
| `src/core/gateway/routes/grpc/routes_mgr.rs` | GlobalGrpcRouteManagers, GrpcPortRouteManager |
| `src/core/gateway/routes/grpc/match_engine.rs` | GrpcMatchEngine (service/method routing) |
| `src/core/gateway/routes/grpc/match_unit.rs` | GrpcRouteRuleUnit, gRPC deep_match |
| `src/core/gateway/routes/grpc/integration.rs` | try_match_grpc_route integration helper |
| `src/core/gateway/runtime/handler.rs` | GatewayHandler (triggers HTTP/gRPC route rebuild on Gateway changes) |
| `src/core/gateway/runtime/server/base.rs` | GatewayBase, listener bootstrap and port-level GatewayInfo collection |
| `src/core/gateway/runtime/server/listener_builder.rs` | ListenerContext, add_http_listener |
| `src/core/gateway/runtime/store/config.rs` | GatewayInfo, GatewayConfigStore |
| `src/core/gateway/runtime/store/port_gateway_info.rs` | PortGatewayInfoStore (port → GatewayInfo mapping) |
| `src/core/gateway/runtime/matching/route.rs` | check_gateway_listener_match, hostname matching |
| `src/core/common/matcher/host_match/radix_match/radix_host_match.rs` | RadixHostMatchEngine (wildcard domain matching) |
| `src/types/resources/http_route.rs` | HTTPRouteSpec (includes resolved_ports) |
| `src/types/resources/grpc_route.rs` | GRPCRouteSpec (includes resolved_ports) |
| `src/types/resources/common.rs` | ParentReference type definition |
| `src/types/ctx.rs` | EdgionHttpContext (per-request state) |
