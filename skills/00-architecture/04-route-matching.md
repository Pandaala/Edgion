# Route Matching Architecture

> Deep reference for HTTP/gRPC route matching in Edgion Gateway.
> Covers the full data flow from config registration to per-request matching,
> including the global route table design, match engines, and listener-level
> multi-Gateway support.

## Overview

The route matching system resolves incoming HTTP/gRPC requests to backend
services through a multi-level matching pipeline:

```
Request (port, hostname, path, headers, query, method)
  → Listener (physical TCP binding, one per port)
    → Global route table lookup:
      → Domain match (exact → wildcard → catch-all)
        → Path match (regex → radix tree: exact → prefix)
          → Deep match (method, headers, query params, Gateway/Listener constraints)
            → RouteMatchResult { route_unit, matched_gateway }
```

All gateways sharing a port share a single global route table.
Gateway/Listener ownership is validated during `deep_match`, not during
domain or path lookup.

## Key Data Structures

### RouteManager (global singleton)

```
RouteManager
├── global_routes: ArcSwap<DomainRouteRules>
│   Single route table for ALL gateways.
│   Routes carry parentRef → Gateway binding; gateway validation
│   happens during deep_match via caller-supplied gateway_infos.
│
└── http_routes: Mutex<HashMap<RouteKey, HTTPRoute>>
    Stores all HTTPRoute resources for lookup during delete/update events.
```

**Key file:** `src/core/routes/http_routes/routes_mgr.rs`

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

**Key file:** `src/core/routes/http_routes/match_unit.rs`

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

**Key file:** `src/core/routes/http_routes/match_engine/radix_route_match.rs`

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

**Key file:** `src/core/routes/http_routes/match_engine/regex_routes_engine.rs`

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

**Key file:** `src/core/routes/http_routes/match_engine/radix_path.rs`

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

**Key file:** `src/core/gateway/gateway/route_match.rs`

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

**Key file:** `src/core/gateway/gateway/config_store.rs`

## Multi-Gateway Port Sharing

### Architecture

Multiple Kubernetes Gateway resources can declare the same port. Pingora
binds one physical listener per port. All Gateways on the same port share
a single `EdgionHttp` instance carrying `Arc<Vec<GatewayInfo>>` — the
full list of Gateway/Listener contexts for that port.

```
gateway_base.rs:
  1. Pre-collect GatewayInfo per port:
     port_gateway_infos: HashMap<u16, Vec<GatewayInfo>>

  2. For each unique port, create one ListenerContext with all GatewayInfos:
     ListenerContext {
       gateway_infos: Arc<Vec<GatewayInfo>>,  // all gateways on this port
       ...
     }

  3. listener_builder creates EdgionHttp:
     EdgionHttp {
       gateway_infos: Arc<Vec<GatewayInfo>>,  // passed to match_route
       ...
     }
```

### Request Flow

```
pg_request_filter():
  gateway_infos = &edgion_http.gateway_infos

  1. Try gRPC match (if applicable):
     grpc_routes.match_route(session, gateway_infos, hostname)
     → Sets ctx.gateway_info from matched GatewayInfo

  2. Try HTTP match:
     global_routes.match_route(session, ctx, gateway_infos)
     → Returns RouteMatchResult { route_unit, matched_gateway }
     → Sets ctx.gateway_info = result.matched_gateway
     → Sets ctx.route_unit = Some(result.route_unit)

  3. No fallback needed — single unified lookup handles all gateways
```

**Key file:** `src/core/routes/http_routes/proxy_http/pg_request_filter.rs`

## Route Registration (Config Time)

### HTTPRoute → RouteManager flow

```
ConfigClient receives HTTPRoute
  → ConfHandler<HTTPRoute>.full_set() or .partial_update()
    → RouteManager stores HTTPRoute in http_routes map
    → parse_http_routes_to_domain_rules():
        for each HTTPRoute:
          validate(route) → parent_refs, rules, namespace, name
          effective_hostnames = resolve_all_effective_hostnames()
          for each hostname:
            for each rule + match:
              create HttpRouteRuleUnit (or regex variant)
              add to domain_rules_map[hostname]
    → Build RouteRules (RadixRouteMatchEngine + RegexRoutesEngine) per hostname
    → Atomically replace global_routes via ArcSwap::store()
```

### Effective Hostname Resolution

Per Gateway API spec, if HTTPRoute has no `spec.hostnames`:
1. Inherit hostname from the listener specified by `parentRef.sectionName`
2. If no sectionName, use first listener's hostname
3. If listener has no hostname, fall back to catch-all `"*"`

**Key function:** `resolve_all_effective_hostnames()` in `conf_handler_impl.rs`

### Partial Update Strategy

- **Exact domains**: Fine-grained RCU (Read-Copy-Update) — clone HashMap, update
  affected hostnames, atomically swap via inner ArcSwap
- **Wildcard domains**: Rebuild RadixHostMatchEngine, but reuse `Arc<RouteRules>`
  for unaffected hostnames (Arc reuse optimization)
- **Atomicity**: `full_set` replaces the entire `Arc<DomainRouteRules>`;
  `partial_update` swaps only inner `ArcSwap` fields

## gRPC Route Matching

gRPC routes follow a parallel architecture:

```
GrpcRouteManager
├── global_grpc_routes: ArcSwap<DomainGrpcRouteRules>
└── grpc_routes: Mutex<HashMap<RouteKey, GRPCRoute>>

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
validation, identical to HTTP routes.

**Key files:** `src/core/routes/grpc_routes/`

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
  │   ├─ HTTPS isolation check (SNI/Host mismatch → 421)
  │   │
  │   ├─ gRPC route match (if applicable)
  │   │   └─ try_match_grpc_route(grpc_routes, session, ctx, gateway_infos)
  │   │       → on match: ctx.gateway_info = matched GatewayInfo
  │   │
  │   ├─ HTTP route match (if gRPC not matched)
  │   │   └─ global_routes.match_route(session, ctx, gateway_infos)
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
| `src/core/routes/http_routes/routes_mgr.rs` | RouteManager, DomainRouteRules, RouteRules |
| `src/core/routes/http_routes/match_unit.rs` | HttpRouteRuleUnit, RouteMatchResult, deep_match |
| `src/core/routes/http_routes/conf_handler_impl.rs` | HTTPRoute → RouteManager registration |
| `src/core/routes/http_routes/match_engine/radix_route_match.rs` | RadixRouteMatchEngine |
| `src/core/routes/http_routes/match_engine/radix_path.rs` | RadixPath compilation |
| `src/core/routes/http_routes/match_engine/regex_routes_engine.rs` | RegexRoutesEngine |
| `src/core/routes/http_routes/proxy_http/pg_request_filter.rs` | Request filter + route matching invocation |
| `src/core/routes/http_routes/proxy_http/mod.rs` | EdgionHttp struct (gateway_infos) |
| `src/core/routes/grpc_routes/routes_mgr.rs` | GrpcRouteManager, DomainGrpcRouteRules |
| `src/core/routes/grpc_routes/match_engine.rs` | GrpcMatchEngine (service/method routing) |
| `src/core/routes/grpc_routes/match_unit.rs` | GrpcRouteRuleUnit, gRPC deep_match |
| `src/core/routes/grpc_routes/integration.rs` | try_match_grpc_route integration helper |
| `src/core/gateway/gateway_base.rs` | GatewayBase, port-level GatewayInfo collection |
| `src/core/gateway/listener_builder.rs` | ListenerContext, add_http_listener |
| `src/core/gateway/gateway/config_store.rs` | GatewayInfo, GatewayConfigStore |
| `src/core/gateway/gateway/route_match.rs` | check_gateway_listener_match, hostname matching |
| `src/core/matcher/host_match/radix_match.rs` | RadixHostMatchEngine (wildcard domain matching) |
| `src/types/resources/common.rs` | ParentReference type definition |
| `src/types/ctx.rs` | EdgionHttpContext (per-request state) |
