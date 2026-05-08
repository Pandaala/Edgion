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

## Route management

`GlobalTcpRouteManagers` manages global TCP routes following the unified per-listener pattern:

```
GlobalTcpRouteManagers
├── route_cache: DashMap<String, Arc<TCPRoute>>        # global route cache
└── by_listener: DashMap<ListenerKey, Arc<TcpPortRouteManager>>
    └── TcpPortRouteManager
        └── route_table: ArcSwap<TcpRouteTable>        # per-listener snapshot
```

### Listener bucketing

`bucket_routes_by_listener` distributes routes to the matching listener via `resolved_listeners` (precomputed by the Controller). If a route has no `resolved_listeners`, a warning is logged and the route is skipped (TCP routes must bind to a specific listener).

### Rebuild strategy

- `rebuild_all_listener_managers`: full rebuild of every listener route table; cleans up stale listeners with no routes.
- `rebuild_affected_listener_managers`: rebuild only affected listeners (used by incremental updates).

## TCP proxy

`EdgionTcpProxy` is the core TCP proxy implementation:

- Holds an `Arc<TcpPortRouteManager>` and gets the current snapshot via `load_route_table()`.
- Calls `match_route()` once per connection to obtain the route.
- Selects a backend from the route's `backend_refs` and establishes an upstream TCP connection.
- Forwards data transparently in both directions, never parsing the application layer.

## ConfHandler handling

`ConfHandler<TCPRoute>` implementation:

- `full_set`: clear route_cache, full replace, call `rebuild_all_listener_managers`.
- `partial_update`: compute affected listeners, update route_cache, call `rebuild_affected_listener_managers`.

The route's `ArcSwap` atomic swap ensures existing connections are unaffected and new connections automatically pick up the latest snapshot.

## Key source files

| File | Responsibility |
|------|----------------|
| `src/core/gateway/routes/tcp/routes_mgr.rs` | GlobalTcpRouteManagers, TcpPortRouteManager |
| `src/core/gateway/routes/tcp/tcp_route_table.rs` | TcpRouteTable |
| `src/core/gateway/routes/tcp/edgion_tcp.rs` | EdgionTcpProxy |
| `src/core/gateway/routes/tcp/conf_handler_impl.rs` | ConfHandler<TCPRoute> implementation |
