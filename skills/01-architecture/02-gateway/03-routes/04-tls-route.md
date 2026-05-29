---
name: gateway-tls-route
description: TLS route — SNI matching, per-listener route table, terminate vs passthrough proxies.
---

# TLS route

## Overview

TLSRoute implements the SNI-based layer-4 routing capability of the Gateway API. It uses SNI (Server Name Indication) for hostname-based routing and supports two listener modes, selected by the Gateway Listener's `tls.mode`:

- **Terminate** (default): the Gateway terminates TLS and forwards plain TCP to the backend. SNI is read from the completed TLS handshake.
- **Passthrough**: the Gateway does not terminate TLS; it parses SNI from the raw ClientHello and transparently relays all bytes (including the original ClientHello) to the backend.

## Module structure

```
src/core/gateway/routes/tls/
├── mod.rs                  # module entry and type exports
├── conf_handler_impl.rs    # ConfHandler<TLSRoute> implementation
├── routes_mgr.rs           # TlsRouteManager / GlobalTlsRouteManagers
├── gateway_tls_routes.rs   # TlsRouteTable: per-listener TLS route table (SNI-indexed)
├── proxy.rs                # EdgionTlsTcpProxy: TLS-terminate-to-TCP proxy
├── passthrough_proxy.rs    # EdgionTlsPassthroughProxy: TLS passthrough proxy (no termination)
└── sni_parser.rs           # raw-ClientHello SNI extraction (used by passthrough mode)
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

## Route management, bucketing, rebuild, ConfHandler

`GlobalTlsRouteManagers` (`route_cache` + `by_listener → TlsRouteManager → ArcSwap<TlsRouteTable>`) follows the shared L4 per-listener management pattern — listener bucketing via `resolved_listeners`, `rebuild_all`/`rebuild_affected`, and `full_set`/`partial_update` ConfHandler semantics. The TLS delta is the SNI-indexed `TlsRouteTable` (above) instead of a flat `Vec`. See [SKILL.md → Shared L4 per-listener management](SKILL.md#shared-l4-per-listener-management-tcp--tls--udp).

## TLS proxies

TLSRoute is served by two distinct proxy implementations; the listener `tls.mode` selects which one `add_listener` builds (`listener_builder.rs`: `add_tls_terminate_to_tcp_listener` vs `add_tls_passthrough_listener`).

### EdgionTlsTcpProxy (Terminate mode, `proxy.rs`)

TLS-terminate-to-TCP proxy. Doc-comment: "TLS-to-TCP proxy service for Gateway TLS listeners."

- Holds an `Arc<TlsRouteManager>` and gets the current snapshot via `load_route_table()`.
- **Terminates** the TLS handshake; extracts the SNI from the established connection via `ssl_ref.servername(NameType::HOST_NAME)` and tracks `is_mtls` in `TlsContext`.
- Calls `TlsRouteTable::match_route(sni_hostname)` to select a route.
- Selects a backend from the route's `backend_refs` and establishes an upstream **plain TCP** connection.
- Relays decrypted traffic bidirectionally (downstream is TLS-terminated, upstream is TCP).

### EdgionTlsPassthroughProxy (Passthrough mode, `passthrough_proxy.rs`)

TLS passthrough proxy. Doc-comment: "Forwards raw TLS traffic to backends without termination."

- Does **not** terminate TLS. Accumulates the ClientHello record and extracts SNI via `sni_parser::extract_sni` (no handshake performed).
- Matches a route by SNI, selects a backend, and transparently relays all bytes — including the original ClientHello — to the upstream.
- Because TLS is not terminated, there is no `is_mtls` context (the passthrough call site passes `is_mtls = false`).

Both proxies invoke the Stage-2 stream plugins via `run_tls_route_plugins` (see `13-stream-plugin-dev.md`).

## Relationship with the TLS termination subsystem

TLSRoute (Terminate/Passthrough) and the HTTP/gRPC TLS subsystem (the `tls/` module) serve different scenarios:

- **TLSRoute Terminate**: the Gateway terminates TLS and forwards **plain TCP** to the backend (layer-4, TLSRoute semantics).
- **TLSRoute Passthrough**: the Gateway forwards encrypted TLS unchanged to the backend, routing by SNI.
- **HTTP/gRPC TLS subsystem** (`HTTPS` listeners): the Gateway terminates TLS, decrypts, and routes by HTTP/gRPC.

## Key source files

| File | Responsibility |
|------|----------------|
| `src/core/gateway/routes/tls/routes_mgr.rs` | GlobalTlsRouteManagers, TlsRouteManager |
| `src/core/gateway/routes/tls/gateway_tls_routes.rs` | TlsRouteTable (SNI matching) |
| `src/core/gateway/routes/tls/proxy.rs` | EdgionTlsTcpProxy (TLS-terminate-to-TCP) |
| `src/core/gateway/routes/tls/passthrough_proxy.rs` | EdgionTlsPassthroughProxy (TLS passthrough) |
| `src/core/gateway/routes/tls/sni_parser.rs` | raw-ClientHello SNI extraction (passthrough) |
| `src/core/gateway/routes/tls/conf_handler_impl.rs` | ConfHandler<TLSRoute> implementation |
