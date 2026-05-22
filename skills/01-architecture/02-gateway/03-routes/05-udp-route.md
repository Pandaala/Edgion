---
name: gateway-udp-route
description: UDP route — per-listener route table, stateless proxy, ConfHandler.
---

# UDP route

## Overview

UDPRoute implements the L4 UDP routing capability of the Gateway API. UDP routing is the simplest route type — connectionless and without a hostname matching dimension; each listener is bound to at most one route and uses first-match semantics to select the backend.

## Module structure

```
src/core/gateway/routes/udp/
├── mod.rs                # module entry and type exports
├── conf_handler_impl.rs  # ConfHandler<UDPRoute> implementation
├── routes_mgr.rs         # UdpPortRouteManager / GlobalUdpRouteManagers
├── udp_route_table.rs    # UdpRouteTable: per-listener immutable route snapshot
└── edgion_udp.rs         # EdgionUdpProxy: UDP proxy implementation
```

## Per-listener route table

`UdpRouteTable` has the same structure as `TcpRouteTable`:

```rust
pub struct UdpRouteTable {
    routes: Vec<Arc<UDPRoute>>,
}
```

- **Build**: `from_routes` collects all routes from `HashMap<String, Arc<UDPRoute>>`.
- **Match**: `match_route()` uses first-match semantics, returning `routes.first()`.
- No hostname dimension (UDP has no SNI-like mechanism).
- Per the Gateway API spec, each UDP listener is typically bound to at most one UDPRoute.

## Route management

`GlobalUdpRouteManagers` follows the unified per-listener management pattern:

```
GlobalUdpRouteManagers
├── route_cache: DashMap<String, Arc<UDPRoute>>        # global route cache
└── by_listener: DashMap<ListenerKey, Arc<UdpPortRouteManager>>
    └── UdpPortRouteManager
        └── route_table: ArcSwap<UdpRouteTable>        # per-listener snapshot
```

### Listener bucketing

Routes are distributed to the matching listener via `resolved_listeners` (Controller-precomputed). Routes without `resolved_listeners` log a warning and are skipped.

### Rebuild strategy

- `rebuild_all_listener_managers`: full rebuild of every listener route table; cleans up stale listeners with no routes.
- `rebuild_affected_listener_managers`: rebuild only affected listeners (used by incremental updates).

## Stateless UDP proxy

`EdgionUdpProxy` is the core UDP proxy implementation:

- Holds an `Arc<UdpPortRouteManager>` and gets the current snapshot via `load_route_table()` (per-packet hot path).
- Calls `match_route()` for each datagram to obtain the route.
- Selects a backend from the route's `backend_refs`.
- UDP is connectionless — each datagram is forwarded independently with no connection state.
- Supports weighted backend selection (same `BackendSelector` mechanism as TCP / HTTP).

## ConfHandler handling

`ConfHandler<UDPRoute>` is implemented identically to TCPRoute:

- `full_set`: clear route_cache, full replace, call `rebuild_all_listener_managers`.
- `partial_update`: compute affected listeners, update route_cache, call `rebuild_affected_listener_managers`.

The route's `ArcSwap` atomic swap ensures in-flight datagram handling is unaffected.

## Key source files

| File | Responsibility |
|------|----------------|
| `src/core/gateway/routes/udp/routes_mgr.rs` | GlobalUdpRouteManagers, UdpPortRouteManager |
| `src/core/gateway/routes/udp/udp_route_table.rs` | UdpRouteTable |
| `src/core/gateway/routes/udp/edgion_udp.rs` | EdgionUdpProxy |
| `src/core/gateway/routes/udp/conf_handler_impl.rs` | ConfHandler<UDPRoute> implementation |
