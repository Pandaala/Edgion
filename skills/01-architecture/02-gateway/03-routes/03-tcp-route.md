---
name: gateway-tcp-route
description: TCP route — per-listener route table, TCP proxy implementation, ConfHandler.
---

# TCP route

## Overview

TCPRoute implements the L4 TCP routing capability of the Gateway API. TCP routing semantics are much simpler than HTTP/gRPC — there is no hostname or path matching dimension; each listener is bound to a set of routes and uses first-match semantics to select the backend.

## Module structure

```
edgion-gateway/src/routes/tcp/
├── mod.rs                # module entry and type exports
├── conf_handler_impl.rs  # ConfHandler<TCPRoute> implementation
├── routes_mgr.rs         # TcpPortRouteManager / GlobalTcpRouteManagers
├── route_unit.rs         # compiled TcpRouteUnit companion
├── tcp_route_table.rs    # TcpRouteTable: per-listener immutable route snapshot
└── edgion_tcp.rs         # EdgionTcpProxy: TCP proxy implementation
```

## Per-listener route table

`TcpRouteTable` is a minimal immutable snapshot:

```rust
pub struct TcpRouteTable {
    routes: Vec<Arc<TcpRouteUnit>>,
}
```

- **Build**: `from_units` Arc-clones cached compiled units into a Vec and sorts by resource key (`namespace/name`) for deterministic selection.
- **Match**: `match_route()` uses first-match semantics — clones the first unit's Arc.
- TCP has no hostname dimension (unlike TLS's SNI matching), so no `Rfc6125HostnameMatcher` index is needed.
- Per the Gateway API spec, each TCP listener is typically bound to at most one TCPRoute.

## Route management, bucketing, rebuild, ConfHandler

`GlobalTcpRouteManagers` caches `Arc<TcpRouteUnit>` by resource key, with
`by_listener → TcpPortRouteManager → ArcSwap<TcpRouteTable>` for publication.
The handler compiles each incoming route generation once. `full_set` clones the
incoming `Arc<TCPRoute>` handle; `partial_update` moves incoming handles through
the shared L4 delta helper. Neither path deep-copies the immutable schema.
The unit retains that schema Arc and owns the compiled backend selector,
stream-plugin store key, and keepalive configuration.

Listener bucketing uses `resolved_listeners`. Full replacement rebuilds all
listener tables; deltas rebuild only affected listeners under the existing
rebuild lock. Rebuilds reuse cached units, preserving unchanged siblings' WRR
cursors. Old snapshots retain old units; new schema generations create new
units. Empty/removed listener managers are cleared, including handles retained
by proxies. See the [shared L4 orchestration](SKILL.md#shared-l4-per-listener-management-tcp--tls--udp)
for bucketing and clear-pass mechanics, and the
[schema-sharing regression guard](../../../04-review/routing/regression-guards/tcp-route-schema-arc-sharing.md).

## TCP proxy

`EdgionTcpProxy` is the core TCP proxy implementation:

- Holds an `Arc<TcpPortRouteManager>` and gets the current snapshot via `load_route_table()`.
- Calls `match_route()` once per connection to obtain the compiled unit and its schema.
- Selects a backend through the unit's compiled `backend_finder` and establishes an upstream TCP connection after the existing backend checks.
- Forwards data transparently in both directions, never parsing the application layer.

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

The endpoint selector retains live degradation and existing address-safety,
connect timeout, outlier accounting and relay behavior. No retry or upstream TLS
is added. Route-stage stream-plugin evaluation retains its existing ordering.

## Key source files

| File | Responsibility |
|------|----------------|
| `edgion-gateway/src/routes/tcp/routes_mgr.rs` | GlobalTcpRouteManagers, TcpPortRouteManager |
| `edgion-gateway/src/routes/tcp/tcp_route_table.rs` | TcpRouteTable |
| `edgion-gateway/src/routes/tcp/route_unit.rs` | TcpRouteUnit and compiled per-route state |
| `edgion-gateway/src/routes/tcp/edgion_tcp.rs` | EdgionTcpProxy |
| `edgion-gateway/src/routes/tcp/conf_handler_impl.rs` | ConfHandler<TCPRoute> implementation |
