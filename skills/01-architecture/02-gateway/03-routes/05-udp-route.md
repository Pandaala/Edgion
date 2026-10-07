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
    routes: Vec<Arc<UdpRouteUnit>>,
}
```

- **Build**: the ConfHandler compiles each schema resource into one
  `UdpRouteUnit`; `from_units` clones the cached units into the listener snapshot.
- **Match**: `match_route()` uses first-match semantics, returning `routes.first()`.
- No hostname dimension (UDP has no SNI-like mechanism).
- Per the Gateway API spec, each UDP listener is typically bound to at most one UDPRoute.

## Route management, bucketing, rebuild, ConfHandler

`GlobalUdpRouteManagers` (`route_cache: DashMap<String, Arc<UdpRouteUnit>>` +
`by_listener → UdpPortRouteManager → ArcSwap<UdpRouteTable>`) follows the shared
L4 per-listener management pattern — identical bucketing/rebuild/ConfHandler
mechanics to TCPRoute. See [SKILL.md → Shared L4 per-listener management](SKILL.md#shared-l4-per-listener-management-tcp--tls--udp).

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

## Compiled backend boundary

The unit privately couples its actual schema Arc, cold compiled Service targets
and original-index selector. Only the owning Route adapter constructs that
generation; the connection path borrows a selected slot and its checked numeric
Service section, without raw reference cloning or Service-key formatting. All
original weights and occurrences remain, including invalid/denied failure slots.
Denial is checked first. Invalid descriptors (including missing or invalid declared
ports) terminate with `UnsupportedBackend` before discovery; a valid Service whose
live section/endpoints are unavailable retains the existing runtime failure.

Compilation rejects malformed source name/namespace before annotation-derived
runtime or plugin demand. Full publication uses successful units only. Deltas
remove every touched publication key and add only final surviving cache entries,
matching the shared remove/update/add order. Same-key add vacates the old unit
before initialization. Current listener handles are cleared when their route is
withdrawn, while selected old Arcs retain their generation. Raw manager keys keep
omitted namespaces as bare names; canonical backend namespaces default to
`default`. This does not claim atomicity across separate stores or listeners.

The new-session path alone consumes the compiled target. Existing sessions retain
their pinned socket address, activity/retirement rules and expiry; they gain no
backend proof pin or generation-triggered revocation. No retry is added.

## Key source files

| File | Responsibility |
|------|----------------|
| `edgion-gateway/src/routes/udp/routes_mgr.rs` | GlobalUdpRouteManagers, UdpPortRouteManager |
| `edgion-gateway/src/routes/udp/udp_route_table.rs` | UdpRouteTable |
| `edgion-gateway/src/routes/udp/edgion_udp.rs` | EdgionUdpProxy |
| `edgion-gateway/src/routes/udp/conf_handler_impl.rs` | ConfHandler<UDPRoute> implementation |
