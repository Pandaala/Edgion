---
name: gateway-udp-route
description: UDP route — per-listener route table, stateless proxy, ConfHandler.
---

# UDP route

## Overview

UDPRoute implements the L4 UDP routing capability of the Gateway API. UDP routing is the simplest route type — connectionless and without a hostname matching dimension; each listener is bound to at most one route and uses first-match semantics to select the backend.

## Module structure

```
edgion-gateway/src/routes/udp/
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

## Route management, bucketing, rebuild, ConfHandler

`GlobalUdpRouteManagers` (`route_cache` + `by_listener → UdpPortRouteManager → ArcSwap<UdpRouteTable>`) follows the shared L4 per-listener management pattern — identical bucketing/rebuild/ConfHandler mechanics to TCPRoute. See [SKILL.md → Shared L4 per-listener management](SKILL.md#shared-l4-per-listener-management-tcp--tls--udp).

## Stateless UDP proxy

`EdgionUdpProxy` is the core UDP proxy implementation:

- Holds an `Arc<UdpPortRouteManager>` and gets the current snapshot via `load_route_table()` (per-packet hot path).
- Calls `match_route()` for each datagram to obtain the route.
- Selects a backend from the route's `backend_refs`.
- UDP is connectionless — each datagram is forwarded independently with no connection state.
- Supports weighted backend selection (same `BackendSelector` mechanism as TCP / HTTP).

## Key source files

| File | Responsibility |
|------|----------------|
| `edgion-gateway/src/routes/udp/routes_mgr.rs` | GlobalUdpRouteManagers, UdpPortRouteManager |
| `edgion-gateway/src/routes/udp/udp_route_table.rs` | UdpRouteTable |
| `edgion-gateway/src/routes/udp/edgion_udp.rs` | EdgionUdpProxy |
| `edgion-gateway/src/routes/udp/conf_handler_impl.rs` | ConfHandler<UDPRoute> implementation |
