# Route Architecture Consistency Review

Cross-route consistency checklist and reference for TCP, UDP, TLS, HTTP, and gRPC
route implementations. Use this when reviewing route changes or adding new route
types to ensure architectural alignment.

## Architecture Comparison Matrix

| Dimension | TCP | UDP | TLS | HTTP | gRPC |
|-----------|-----|-----|-----|------|------|
| **Data-plane manager** | `GlobalTcpRouteManagers` | `GlobalUdpRouteManagers` | `GlobalTlsRouteManagers` | `RouteManager` | `GrpcRouteManager` |
| **Index dimension** | port | port | port + hostname (SNI) | hostname | hostname (flat) |
| **Per-port manager** | `TcpPortRouteManager` | `UdpPortRouteManager` | `TlsRouteManager` | N/A | N/A |
| **Route table snapshot** | `TcpRouteTable` | `UdpRouteTable` | `TlsRouteTable` | `RouteRules` | `GrpcRouteRules` |
| **Lock-free read** | `ArcSwap` | `ArcSwap` | `ArcSwap` | `ArcSwap` | `ArcSwap` |
| **`resolved_ports`** | Yes | Yes | Yes | No (hostname) | No (hostname) |
| **`gateway_route_index`** | Yes | Yes | Yes | Yes | Yes |
| **`attached_route_tracker`** | Yes | Yes | Yes | Yes | Yes |
| **Data-plane tracing** | None | None | None | None (init only) | None |
| **Access log all events** | Yes | Yes | Yes (if enabled) | Yes | Yes |
| **ConfHandler partial** | port-based rebuild | port-based rebuild | port-based rebuild | hostname-based rebuild | full rebuild |

## Controller Handler Checklist

Every route handler (`ProcessorHandler<T>`) must implement:

| Method | Required behavior |
|--------|-------------------|
| `validate` | `validate_*_route_if_enabled` + `validate_backend_refs` |
| `parse` | `record_cross_ns_refs`, `register_service_refs`, mark `ref_denied`, resolve `resolved_ports` or `resolved_hostnames` |
| `on_change` | `update_gateway_route_index` (if `parse()` calls `lookup_gateway()`), `update_attached_route_tracker`, conditional `requeue_parent_gateways` |
| `on_delete` | `clear_resource_refs`, `clear_service_backend_refs`, `remove_from_gateway_route_index`, `remove_from_attached_route_tracker`, conditional `requeue_parent_gateways` |
| `update_status` | `set_parent_conditions_full` per parent, `retain_current_parent_statuses`, clear when `parent_refs` is None |

**Critical rule:** Any handler that calls `lookup_gateway()` in `parse()` MUST
register in `gateway_route_index` via `on_change()`. See `skills/gateway-api/SKILL.md`.

## Access Log Consistency

| Protocol | Log struct | Trigger | Covers failures? |
|----------|-----------|---------|------------------|
| TCP | `TcpLogEntry` | Per connection (on disconnect) | Yes — all status codes |
| UDP | `UdpLogEntry` | Per session (on timeout) + per-packet (on failure) | Yes |
| TLS | `TlsContext` (serialized) | Per connection (on disconnect) | Yes (if `tls_proxy_log_record` enabled) |
| HTTP | `AccessLogEntry` | Per request | Yes |
| gRPC | `AccessLogEntry` | Per request (shared HTTP pipeline) | Yes |

## Data-Plane Design Rules

1. **No `tracing::*` in hot path** — match/proxy functions must not call tracing macros
2. **ArcSwap for route tables** — all route types use `ArcSwap` for lock-free concurrent reads
3. **ConfHandler rebuild** — `full_set` always full-rebuild; `partial_update` rebuilds only affected dimension (ports or hostnames)
4. **Stats for leak detection** — every manager exposes `stats()` → `*Stats` struct used by `/api/v1/debug/store-stats`

## Known Differences (Acceptable)

These differences exist by design and do not need unification:

1. **HTTP/gRPC use hostname indexing, not port** — they bind to shared HTTP listeners where hostname is the discriminator
2. **TLS has hostname (SNI) matching within per-port tables** — combines port + hostname dimension
3. **TLS has dual logging (global + per-listener)** — historical compatibility
4. **gRPC partial_update rebuilds entire table** — route count is typically small; hostname bucketing would add complexity without proportional benefit
5. **HTTP/gRPC use `resolved_hostnames`, TCP/UDP/TLS use `resolved_ports`** — different parentRef resolution semantics

## Improvement Backlog

Items identified during review that could improve consistency:

| # | Item | Priority | Status |
|---|------|----------|--------|
| 1 | Consider port-dimension indexing for HTTP/gRPC (multi-port gateway isolation) | P3 | Design needed — see analysis below |

### HTTP/gRPC Port-Dimension Analysis

**Current:** HTTP and gRPC routes are matched purely by hostname. A single global
`RouteManager` / `GrpcRouteManager` serves all ports. This means that if the same
hostname is configured on two listeners with different ports, both listeners see the
same route set.

**Tradeoff:** Adding port-dimension would enable per-port route isolation (matching
Gateway API's listener-level scoping). However, HTTP/gRPC already resolve hostnames
at the controller level, and the Pingora HTTP proxy receives requests after TLS
termination where port information is available but not currently used for routing.

**Difficulty:** Medium-High. Would require:
- Adding `resolved_ports` to HTTPRoute/GRPCRoute (controller change)
- Restructuring `RouteManager` to be per-port (similar to TLS pattern)
- Passing listener port through the HTTP proxy pipeline for route matching
- Updating all hostname resolution logic to be port-scoped
- Significant test coverage changes

**Recommendation:** Defer unless multi-port same-hostname isolation becomes a
concrete user requirement.

## File Reference

| Component | TCP | UDP | TLS | HTTP | gRPC |
|-----------|-----|-----|-----|------|------|
| Route table | `routes/tcp/tcp_route_table.rs` | `routes/udp/udp_route_table.rs` | `routes/tls/gateway_tls_routes.rs` | `routes/http/routes_mgr.rs` | `routes/grpc/routes_mgr.rs` |
| Manager | `routes/tcp/routes_mgr.rs` | `routes/udp/routes_mgr.rs` | `routes/tls/routes_mgr.rs` | `routes/http/routes_mgr.rs` | `routes/grpc/routes_mgr.rs` |
| ConfHandler | `routes/tcp/conf_handler_impl.rs` | `routes/udp/conf_handler_impl.rs` | `routes/tls/conf_handler_impl.rs` | `routes/http/conf_handler_impl.rs` | `routes/grpc/conf_handler_impl.rs` |
| Proxy | `routes/tcp/edgion_tcp.rs` | `routes/udp/edgion_udp.rs` | `routes/tls/proxy.rs` | `routes/http/proxy_http/` | `routes/grpc/integration.rs` |
| Controller handler | `handlers/tcp_route.rs` | `handlers/udp_route.rs` | `handlers/tls_route.rs` | `handlers/http_route.rs` | `handlers/grpc_route.rs` |
| Access log | `observe/logs/tcp_log.rs` | `observe/logs/udp_log.rs` | `observe/logs/tls_log.rs` | `observe/access_log/entry.rs` | (shared with HTTP) |
