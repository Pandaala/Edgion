---
name: gateway-tls-route
description: TLS route — SNI matching, per-listener route table, TLS proxy implementation.
---

# TLS route

## Overview

TLSRoute implements the TLS passthrough routing capability of the Gateway API. Unlike TCPRoute, TLSRoute uses SNI (Server Name Indication) during the TLS handshake to do hostname-based routing and forwards the TLS connection unchanged to the backend (without terminating TLS).

## Module structure

```
src/core/gateway/routes/tls/
├── mod.rs                  # module entry and type exports
├── conf_handler_impl.rs    # ConfHandler<TLSRoute> implementation
├── routes_mgr.rs           # TlsRouteManager / GlobalTlsRouteManagers
├── gateway_tls_routes.rs   # TlsRouteTable: per-listener TLS route table (SNI-indexed)
└── proxy.rs                # EdgionTlsTcpProxy: TLS passthrough proxy implementation
```

## SNI matching

`TlsRouteTable` uses the `HashHost` data structure to perform hostname matching by SNI:

```rust
pub struct TlsRouteTable {
    host_map: HashHost<Vec<Arc<TLSRoute>>>,
    catch_all_routes: Option<Vec<Arc<TLSRoute>>>,
}
```

Match priority:

1. **Exact hostname match**: `HashHost` exact lookup, O(1).
2. **Wildcard hostname match**: `HashHost` handles wildcards such as `*.example.com` internally and returns the most specific wildcard match.
3. **Catch-all**: routes without `spec.hostnames` serve as a fallback.

`TlsRouteTable::from_routes` build process:

1. Iterate all routes and bucket by `spec.hostnames` (lowercased).
2. Routes without a hostname go into `catch_all_routes`.
3. Insert the bucketed result into the `HashHost` matcher.

## Per-listener route management

`GlobalTlsRouteManagers` follows the unified per-listener management pattern:

```
GlobalTlsRouteManagers
├── route_cache: DashMap<String, Arc<TLSRoute>>        # global route cache
└── by_listener: DashMap<ListenerKey, Arc<TlsRouteManager>>
    └── TlsRouteManager
        └── route_table: ArcSwap<TlsRouteTable>        # per-listener snapshot
```

### Listener bucketing

Routes are distributed to the matching listener via `resolved_listeners` (Controller-precomputed). Routes without `resolved_listeners` log a warning and are skipped.

### Rebuild strategy

- `rebuild_all_listener_managers`: full rebuild of all listeners; cleans up stale listeners with no routes.
- `rebuild_affected_listener_managers`: rebuild only affected listeners (used by incremental updates).

## TLS passthrough proxy

`EdgionTlsTcpProxy` is the core implementation of the TLS passthrough proxy:

- Holds an `Arc<TlsRouteManager>` and gets the current snapshot via `load_route_table()`.
- Extracts the SNI from ClientHello during the TLS handshake.
- Calls `TlsRouteTable::match_route(sni_hostname)` to select a route.
- Selects a backend from the route's `backend_refs` and establishes an upstream TCP connection.
- Forwards TLS traffic transparently in both directions; TLS is not terminated.

## Relationship with the TLS subsystem

TLSRoute and the TLS subsystem (the `tls/` module) serve different scenarios:

- **TLSRoute**: TLS passthrough — the Gateway does not terminate TLS and routes by SNI to the backend.
- **TLS subsystem**: TLS termination — the Gateway terminates the TLS handshake, decrypts, and routes by HTTP/gRPC.

They can coexist on the same port — the Gateway Listener's `tls.mode` determines which mode is used.

## Key source files

| File | Responsibility |
|------|----------------|
| `src/core/gateway/routes/tls/routes_mgr.rs` | GlobalTlsRouteManagers, TlsRouteManager |
| `src/core/gateway/routes/tls/gateway_tls_routes.rs` | TlsRouteTable (SNI matching) |
| `src/core/gateway/routes/tls/proxy.rs` | EdgionTlsTcpProxy |
| `src/core/gateway/routes/tls/conf_handler_impl.rs` | ConfHandler<TLSRoute> implementation |
