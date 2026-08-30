---
name: gateway-udp-route
description: UDP route — per-listener route table, client-session upstream pinning, and ConfHandler.
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

## UDP client-session proxy

`EdgionUdpProxy` is the core UDP proxy implementation:

- Maintains a per-client `client_sessions` map and pins each active client to one upstream address.
- The first packet for a client loads the current route snapshot, runs route/plugins/backend selection,
  creates the upstream socket/session, and records the selected address.
- Subsequent packets take the session fast path and reuse the pinned upstream without repeating route,
  plugin, or backend selection.
- Sessions expire after 60 seconds of inactivity; each listener is capped at 10,000 sessions.
- Route or backend configuration changes therefore affect an existing client after its pinned session
  expires (or is otherwise removed), not on the next datagram.

## Key source files

| File | Responsibility |
|------|----------------|
| `edgion-gateway/src/routes/udp/routes_mgr.rs` | GlobalUdpRouteManagers, UdpPortRouteManager |
| `edgion-gateway/src/routes/udp/udp_route_table.rs` | UdpRouteTable |
| `edgion-gateway/src/routes/udp/edgion_udp.rs` | EdgionUdpProxy |
| `edgion-gateway/src/routes/udp/conf_handler_impl.rs` | ConfHandler<UDPRoute> implementation |
