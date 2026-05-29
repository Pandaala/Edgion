---
name: gateway-tcp-route
description: TCP route — per-listener route table, TCP proxy implementation, ConfHandler.
---

# TCP route

## Overview

TCPRoute implements the L4 TCP routing capability of the Gateway API. TCP routing semantics are much simpler than HTTP/gRPC — there is no hostname or path matching dimension; each listener is bound to a set of routes and uses first-match semantics to select the backend.

## Module structure

```
src/core/gateway/routes/tcp/
├── mod.rs                # module entry and type exports
├── conf_handler_impl.rs  # ConfHandler<TCPRoute> implementation
├── routes_mgr.rs         # TcpPortRouteManager / GlobalTcpRouteManagers
├── tcp_route_table.rs    # TcpRouteTable: per-listener immutable route snapshot
└── edgion_tcp.rs         # EdgionTcpProxy: TCP proxy implementation
```

## Per-listener route table

`TcpRouteTable` is a minimal immutable snapshot:

```rust
pub struct TcpRouteTable {
    routes: Vec<Arc<TCPRoute>>,
}
```

- **Build**: `from_routes` collects all routes from `HashMap<String, Arc<TCPRoute>>` into a Vec.
- **Match**: `match_route()` uses first-match semantics — returns `routes.first()` directly.
- TCP has no hostname dimension (unlike TLS's SNI matching), so no `HashHost` index is needed.
- Per the Gateway API spec, each TCP listener is typically bound to at most one TCPRoute.

## Route management, bucketing, rebuild, ConfHandler

`GlobalTcpRouteManagers` (`route_cache` + `by_listener → TcpPortRouteManager → ArcSwap<TcpRouteTable>`) follows the shared L4 per-listener management pattern — listener bucketing via `resolved_listeners`, `rebuild_all`/`rebuild_affected`, and `full_set`/`partial_update` ConfHandler semantics. See [SKILL.md → Shared L4 per-listener management](SKILL.md#shared-l4-per-listener-management-tcp--tls--udp).

## TCP proxy

`EdgionTcpProxy` is the core TCP proxy implementation:

- Holds an `Arc<TcpPortRouteManager>` and gets the current snapshot via `load_route_table()`.
- Calls `match_route()` once per connection to obtain the route.
- Selects a backend from the route's `backend_refs` and establishes an upstream TCP connection.
- Forwards data transparently in both directions, never parsing the application layer.

## Key source files

| File | Responsibility |
|------|----------------|
| `src/core/gateway/routes/tcp/routes_mgr.rs` | GlobalTcpRouteManagers, TcpPortRouteManager |
| `src/core/gateway/routes/tcp/tcp_route_table.rs` | TcpRouteTable |
| `src/core/gateway/routes/tcp/edgion_tcp.rs` | EdgionTcpProxy |
| `src/core/gateway/routes/tcp/conf_handler_impl.rs` | ConfHandler<TCPRoute> implementation |
