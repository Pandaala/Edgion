# Route Architecture Consistency Review

Cross-route consistency checklist and reference for TCP, UDP, TLS, HTTP, and gRPC
route implementations. Use this when reviewing route changes or adding new route
types to ensure architectural alignment.

## Architecture Comparison Matrix

### Data-Plane Structure

| Dimension | TCP | UDP | TLS | HTTP | gRPC |
|-----------|-----|-----|-----|------|------|
| **Global manager** | `GlobalTcpRouteManagers` | `GlobalUdpRouteManagers` | `GlobalTlsRouteManagers` | `GlobalHttpRouteManagers` | `GlobalGrpcRouteManagers` |
| **Global singleton** | `OnceLock` | `OnceLock` | `OnceLock` | `OnceLock` | `OnceLock` |
| **Legacy alias** | — | — | — | `RouteManager` | `GrpcRouteManager` |
| **Per-port manager** | `TcpPortRouteManager` | `UdpPortRouteManager` | `TlsRouteManager` | `HttpPortRouteManager` | `GrpcPortRouteManager` |
| **Route table snapshot** | `TcpRouteTable` | `UdpRouteTable` | `TlsRouteTable` | `DomainRouteRules` | `DomainGrpcRouteRules` |
| **Lock-free read** | `ArcSwap` | `ArcSwap` | `ArcSwap` | `ArcSwap` | `ArcSwap` |

### Index Dimensions

| Dimension | TCP | UDP | TLS | HTTP | gRPC |
|-----------|-----|-----|-----|------|------|
| **Port** | Yes | Yes | Yes | Yes | Yes |
| **Hostname** | No | No | Yes (SNI via `HashHost`) | Yes (exact/wildcard/catch-all) | No (flat per-port) |
| **Path** | N/A | N/A | N/A | Yes (radix + regex) | N/A |
| **Service/Method** | N/A | N/A | N/A | N/A | Yes (exact/service/catch-all) |

### Route Cache & Concurrency

| Dimension | TCP | UDP | TLS | HTTP | gRPC |
|-----------|-----|-----|-----|------|------|
| **`route_cache`** | `DashMap<String, Arc<TCPRoute>>` | `DashMap<String, Arc<UDPRoute>>` | `DashMap<String, Arc<TLSRoute>>` | `DashMap<String, HTTPRoute>` | `DashMap<String, GRPCRoute>` |
| **`by_port`** | `DashMap<u16, Arc<...>>` | `DashMap<u16, Arc<...>>` | `DashMap<u16, Arc<...>>` | `DashMap<u16, Arc<...>>` | `DashMap<u16, Arc<...>>` |
| **Extra cache** | — | — | — | — | `route_units_cache: Mutex<HashMap<...>>` |
| **Route table swap** | `ArcSwap<TcpRouteTable>` | `ArcSwap<UdpRouteTable>` | `ArcSwap<TlsRouteTable>` | `ArcSwap<DomainRouteRules>` | `ArcSwap<DomainGrpcRouteRules>` |

### Matching Strategy

| Dimension | TCP | UDP | TLS | HTTP | gRPC |
|-----------|-----|-----|-----|------|------|
| **Match type** | First match | First match | SNI hostname match | Domain → Path → Deep match | Service/Method → Deep match |
| **Match engine** | `Vec::first()` | `Vec::first()` | `HashHost` (exact → wildcard → catch-all) | `RadixRouteMatchEngine` + `RegexRoutesEngine` | `GrpcMatchEngine` (exact → service → catch-all) |
| **Gateway validation** | No | No | No | Yes (`deep_match` with `gateway_infos`) | Yes (`deep_match` with `gateway_infos`) |

### `resolved_ports` Handling

| Dimension | TCP | UDP | TLS | HTTP | gRPC |
|-----------|-----|-----|-----|------|------|
| **Has `resolved_ports`** | Yes | Yes | Yes | Yes | Yes |
| **Fallback when empty** | Skip (warn) | Skip (warn) | Skip (warn) | 3-tier: `parentRef.port` → all known ports | 3-tier: `parentRef.port` → all known ports |
| **Cross-handler rebuild** | No | No | No | Yes (GatewayHandler triggers) | Yes (GatewayHandler triggers) |

### ConfHandler Behavior

| Dimension | TCP | UDP | TLS | HTTP | gRPC |
|-----------|-----|-----|-----|------|------|
| **`full_set`** | Clear → insert all → `rebuild_all_port_managers` | Same as TCP | Same as TCP | Clear → insert all → `rebuild_all_port_managers` | Clear → insert all → `rebuild_all_port_managers` |
| **`partial_update`** | Compute affected ports → update cache → `rebuild_affected_port_managers` | Same as TCP | Same as TCP | Compute affected ports (with fallback) → update cache → `rebuild_affected_port_managers` | Same as HTTP |
| **`initialize_route`** | BackendSelector, stream plugins, annotations | BackendSelector | BackendSelector, proxy protocol, upstream TLS, stream plugins, retries | N/A (parsing in `build_domain_route_rules_from_routes`) | N/A (parsing in `parse_route_to_units`) |
| **LB policy sync** | No | No | No | Yes (`sync_lb_policies_for_routes`) | No |

### Stats

| Dimension | TCP | UDP | TLS | HTTP | gRPC |
|-----------|-----|-----|-----|------|------|
| **`route_cache`** | Yes | Yes | Yes | Yes (`http_routes`) | Yes (`grpc_routes`) |
| **`port_count`** | Yes | Yes | Yes | Yes | Yes |
| **Hostname stats** | — | — | — | `exact_domains`, `wildcard_domains`, `has_catch_all` | — |
| **Extra** | — | — | — | — | `resource_keys`, `route_units_cache` |

---

## Proxy Route Loading (Hot Path)

All five protocols follow the same pattern: load per-port route table → match.

| Protocol | Proxy struct | Route load | Match call |
|----------|-------------|------------|------------|
| **TCP** | `EdgionTcpProxy` | `self.tcp_route_manager.load_route_table()` | `route_table.match_route()` → first |
| **UDP** | `EdgionUdpProxy` | `self.udp_route_manager.load_route_table()` | `route_table.match_route()` → first |
| **TLS** | `EdgionTlsTcpProxy` | `self.tls_route_manager.load_route_table()` | `route_table.match_route(sni)` |
| **HTTP** | `EdgionHttpProxy` | `get_global_http_route_managers().get_or_create_port_manager(port).load_route_table()` | `port_routes.match_route(session, ctx, &gateway_infos)` |
| **gRPC** | `EdgionHttpProxy` (shared) | `get_global_grpc_route_managers().get_or_create_port_manager(port).load_route_table()` | `try_match_grpc_route(&grpc_routes, session, ctx, &gateway_infos)` |

**Key difference:** TCP/UDP/TLS proxies hold `Arc<*PortRouteManager>` as a struct
field (bound at listener creation). HTTP/gRPC look up the per-port manager
dynamically from the global singleton at request time via
`get_or_create_port_manager(port)`. This is because HTTP/gRPC share a single
`EdgionHttpProxy` struct and need dynamic port resolution.

---

## Controller Handler Checklist

Every route handler (`ProcessorHandler<T>`) must implement:

| Method | Required behavior |
|--------|-------------------|
| `validate` | `validate_*_route_if_enabled` + `validate_backend_refs` |
| `parse` | `record_cross_ns_refs`, `register_service_refs`, mark `ref_denied`, resolve `resolved_ports` (and `resolved_hostnames` for L7) |
| `on_change` | `update_gateway_route_index`, `update_attached_route_tracker`, conditional `requeue_parent_gateways` |
| `on_delete` | `clear_resource_refs`, `clear_service_backend_refs`, `remove_from_gateway_route_index`, `remove_from_attached_route_tracker`, conditional `requeue_parent_gateways` |
| `update_status` | `set_parent_conditions_full` per parent, `retain_current_parent_statuses`, clear when `parent_refs` is None |

**Critical rule:** Any handler that calls `lookup_gateway()` in `parse()` MUST
register in `gateway_route_index` via `on_change()`. See `skills/08-gateway-api/SKILL.md`.

### Controller Handler Comparison

| Aspect | TCP | UDP | TLS | HTTP | gRPC |
|--------|-----|-----|-----|------|------|
| `validate()` | `validate_tcp_route_if_enabled` | `validate_udp_route_if_enabled` | `validate_tls_route_if_enabled` | `validate_http_route_if_enabled` | `validate_grpc_route_if_enabled` |
| `validate_backend_refs` | Yes | Yes | Yes | Yes | Yes |
| `record_cross_ns_refs` | Yes | Yes | Yes | Yes | Yes |
| `register_service_refs` | Yes | Yes | Yes | Yes | Yes |
| `ref_denied` marking | Yes | Yes | Yes | Yes | Yes |
| Clear `resolved_ports` before resolve | No | No | No | Yes | Yes |
| Resolve `resolved_ports` | Yes | Yes | Yes | Yes | Yes |
| Resolve `resolved_hostnames` | No (L4) | No (L4) | No | Yes | Yes |
| Hostname resolution annotation | No | No | No | Yes | Yes |
| Warn when `resolved_ports` empty | Yes | Yes | Yes | No | No |
| `update_gateway_route_index` | Yes | Yes | Yes | Yes | Yes |
| `update_attached_route_tracker` | Yes | Yes | Yes | Yes | Yes |
| `requeue_parent_gateways` | Yes | Yes | Yes | Yes | Yes |

---

## Access Log Consistency

| Protocol | Log struct | Trigger | Covers failures? |
|----------|-----------|---------|------------------|
| TCP | `TcpLogEntry` | Per connection (on disconnect) | Yes — all status codes |
| UDP | `UdpLogEntry` | Per session (on timeout) + per-packet (on failure) | Yes |
| TLS | `TlsContext` (serialized) | Per connection (on disconnect) | Yes (if `tls_proxy_log_record` enabled) |
| HTTP | `AccessLogEntry` | Per request | Yes |
| gRPC | `AccessLogEntry` | Per request (shared HTTP pipeline) | Yes |

---

## Data-Plane Design Rules

1. **No `tracing::*` in hot path** — match/proxy functions must not call tracing macros
2. **ArcSwap for route tables** — all route types use `ArcSwap` for lock-free concurrent reads
3. **ConfHandler rebuild** — `full_set` always full-rebuild; `partial_update` rebuilds only affected ports
4. **Stats for leak detection** — every manager exposes `stats()` → `*Stats` struct used by `/api/v1/debug/store-stats`
5. **Per-port isolation** — all five route types use `Global*RouteManagers` with `by_port: DashMap<u16, Arc<*PortRouteManager>>`

---

## Known Differences (Acceptable)

These differences exist by design and do not need unification:

1. **TLS has hostname (SNI) matching within per-port tables** — combines port + hostname dimension; TCP/UDP have no hostname dimension
2. **HTTP has multi-level hostname matching** — exact → wildcard → catch-all within per-port tables; TCP/UDP/TLS have simpler matching
3. **HTTP/gRPC load port managers dynamically** — via `get_or_create_port_manager(port)` at request time, because they share `EdgionHttpProxy`; TCP/UDP/TLS hold `Arc<*PortRouteManager>` in their proxy struct
4. **TLS has dual logging (global + per-listener)** — historical compatibility
5. **gRPC has `route_units_cache` with `Mutex`** — needed because gRPC pre-parses routes into `GrpcRouteRuleUnit` units; HTTP parses inline during route table build
6. **HTTP has LB policy sync** — `sync_lb_policies_for_routes` / `cleanup_lb_policies_for_routes`; other protocols handle LB at `initialize_route` time or via `BackendSelector`
7. **HTTP/gRPC have 3-tier port fallback** — for backward compatibility when `resolved_ports` is not set; TCP/UDP/TLS skip routes without `resolved_ports`

---

## Cross-Handler Rebuild (HTTP/gRPC only)

HTTP and gRPC route full_set/partial_update may execute before or after Gateway
full_set (independent gRPC watch streams, non-deterministic order). To handle this:

`GatewayHandler::full_set()` and `partial_update()` call
`get_global_http_route_managers().rebuild_all_port_managers()` and
`get_global_grpc_route_managers().rebuild_all_port_managers()` after
`rebuild_port_gateway_infos()`.

This ensures routes cached before Gateways arrived are redistributed to the
correct per-port managers once `PortGatewayInfoStore` is populated.

TCP/UDP/TLS do not need this because they skip routes without `resolved_ports`,
and the controller always resolves ports before syncing.

---

## Inconsistencies (Actionable)

| # | Finding | Severity | Affected | Recommendation |
|---|---------|----------|----------|----------------|
| 1 | TCP/UDP/TLS do not clear `resolved_ports` before recompute in `parse()`; HTTP/gRPC do | Medium | Controller handlers | Add `route.spec.resolved_ports = None` at start of `parse()` in TCP/UDP/TLS for consistency |
| 2 | TCP/UDP/TLS warn when `resolved_ports` is empty after `parse()`; HTTP/gRPC do not | Low | Controller handlers | Align: either add warning to HTTP/gRPC or remove from TCP/UDP/TLS |
| 3 | HTTP/gRPC `route_cache` stores values directly (`HTTPRoute`/`GRPCRoute`); TCP/UDP/TLS wrap in `Arc` (`Arc<TCPRoute>`) | Low | Gateway route managers | Cosmetic; no functional impact. Consider unifying to `Arc` for consistency |
| 4 | gRPC `route_units_cache` uses `Mutex`; all other caches use `DashMap` | Low | gRPC route manager | Consider `DashMap` if contention becomes measurable |

---

## Improvement Backlog

| # | Item | Priority | Status |
|---|------|----------|--------|
| 1 | ~~Per-port isolation for HTTP/gRPC~~ | ~~P3~~ | **Done** — implemented per-port `GlobalHttpRouteManagers` / `GlobalGrpcRouteManagers` |
| 2 | Align `resolved_ports` clearing in TCP/UDP/TLS controller handlers | P4 | Open |
| 3 | Align warning behavior for empty `resolved_ports` across all handlers | P4 | Open |
| 4 | Consider `DashMap` for gRPC `route_units_cache` | P5 | Open |

---

## File Reference

| Component | TCP | UDP | TLS | HTTP | gRPC |
|-----------|-----|-----|-----|------|------|
| Global manager | `routes/tcp/routes_mgr.rs` | `routes/udp/routes_mgr.rs` | `routes/tls/routes_mgr.rs` | `routes/http/routes_mgr.rs` | `routes/grpc/routes_mgr.rs` |
| Per-port manager | (same file) | (same file) | (same file) | (same file) | (same file) |
| Route table | `routes/tcp/tcp_route_table.rs` | `routes/udp/udp_route_table.rs` | `routes/tls/gateway_tls_routes.rs` | `routes/http/routes_mgr.rs` | `routes/grpc/routes_mgr.rs` |
| ConfHandler | `routes/tcp/conf_handler_impl.rs` | `routes/udp/conf_handler_impl.rs` | `routes/tls/conf_handler_impl.rs` | `routes/http/conf_handler_impl.rs` | `routes/grpc/conf_handler_impl.rs` |
| Proxy | `routes/tcp/edgion_tcp.rs` | `routes/udp/edgion_udp.rs` | `routes/tls/proxy.rs` | `routes/http/proxy_http/` | `routes/grpc/integration.rs` |
| Controller handler | `handlers/tcp_route.rs` | `handlers/udp_route.rs` | `handlers/tls_route.rs` | `handlers/http_route.rs` | `handlers/grpc_route.rs` |
| Access log | `observe/logs/tcp_log.rs` | `observe/logs/udp_log.rs` | `observe/logs/tls_log.rs` | `observe/access_log/entry.rs` | (shared with HTTP) |
| Runtime handler | — | — | — | `runtime/handler.rs` (cross-handler rebuild) | (same) |
