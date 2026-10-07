---
name: gateway-tls-route
description: TLS route — SNI matching, per-listener route table, terminate vs passthrough proxies.
---

# TLS route

## Overview

TLSRoute implements SNI-based layer-4 routing. The code contains two proxy paths,
but only Passthrough is currently enabled: the Controller rejects explicit
TLS/Terminate listeners with `Accepted=False/UnsupportedValue`. The termination
path below describes retained implementation, not an enabled product feature.

- **Terminate** (deferred): the proxy implementation terminates TLS and forwards plain TCP to the backend. SNI is read from the completed TLS handshake.
- **Passthrough**: the Gateway does not terminate TLS; it parses SNI from the raw ClientHello and transparently relays all bytes (including the original ClientHello) to the backend.

## Module structure

```
edgion-gateway/src/routes/tls/
├── mod.rs                  # module entry and type exports
├── conf_handler_impl.rs    # ConfHandler<TLSRoute> implementation
├── routes_mgr.rs           # TlsRouteManager / GlobalTlsRouteManagers
├── gateway_tls_routes.rs   # TlsRouteTable: per-listener TLS route table (SNI-indexed)
├── proxy.rs                # EdgionTlsTcpProxy: TLS-terminate-to-TCP proxy
├── passthrough_proxy.rs    # EdgionTlsPassthroughProxy: TLS passthrough proxy (no termination)
└── sni_parser.rs           # raw-ClientHello SNI extraction (used by passthrough mode)
```

## SNI matching

`TlsRouteTable` uses the gateway's shared hostname matcher for Route SNI candidates:

```rust
pub struct TlsRouteTable {
    hosts: RadixHostnameMatcher<Vec<TlsRouteCandidate>>,
}
```

Match priority:

1. **Exact hostname match**: authored exact Route patterns first.
2. **Wildcard hostname match**: matching suffixes longest first; `*.example.com` includes deep subdomains but excludes `example.com`.
3. **No match**: reject if no explicit route hostname matches; TLSRoute v1 has no hostname-less catch-all.

`TlsRouteTable::with_units` build process:

1. Group by validated authored Route patterns that intersect accepted
   `ResolvedTlsListener` records on the table's physical `ListenerKey`. Retain
   the contributing attachment indices with each candidate, not a global union.
2. Empty intersections and unresolved/invalid routes contribute no candidates.
3. Sort candidates by oldest creation timestamp, namespace, then name; retain
   runners-up in each published group. Reuse unchanged group Arcs by candidate
   identity/order and attachment indices; reuse the tree when final keys agree.

All SNI slots selecting one route share its compiled unit and backend selector.
Route matching preserves Gateway API multi-depth wildcard matching. EdgionTls
and Gateway TLS/certificate selection remain unchanged and single-label.
Old snapshots retain their selected
units until their readers release them; only the outer manager publishes via
`ArcSwap`.

The Controller resolves names per exact Gateway/listener attachment. Runtime
units retain accepted schema indices and compile exact logical parent identities
per physical key. Selection checks effective hostname and exact logical parent
from the SAME attachment, then existing Listener eligibility; a rejected candidate
can yield to a runner-up or broader Route group, never a less-specific Listener.
Both proxies use one shared route/scope selection path. Once an owner is selected,
unavailable policy, denied backend or connection failure is terminal for that
selection; none restarts hostname iteration. Retain the selected runtime/scope,
not unrelated full snapshots across plugin/dial awaits.
See [attachment regression boundaries](../../../04-review/routing/regression-guards/tls-hostnames-per-attachment.md).

## Route management, bucketing, rebuild, ConfHandler

`GlobalTlsRouteManagers` (`route_cache` + `by_listener → TlsRouteManager → ArcSwap<TlsRouteTable>`) follows the shared L4 per-listener management pattern — listener bucketing via `resolved_listeners`, `rebuild_all`/`rebuild_affected`, and `full_set`/`partial_update` ConfHandler semantics. The TLS delta is the SNI-indexed `TlsRouteTable` (above) instead of a flat `Vec`. See [SKILL.md → Shared L4 per-listener management](SKILL.md#shared-l4-per-listener-management-tcp--tls--udp).

## TLS proxies

TLSRoute is served by two distinct proxy implementations; the listener `tls.mode` selects which one `add_listener` builds (`listener_builder.rs`: `add_tls_terminate_to_tcp_listener` vs `add_tls_passthrough_listener`).

### EdgionTlsTcpProxy (Terminate mode, `proxy.rs`)

TLS-terminate-to-TCP proxy. Doc-comment: "TLS-to-TCP proxy service for Gateway TLS listeners."

- Holds an `Arc<TlsRouteManager>` and gets the current snapshot via `load_route_table()`.
- **Terminates** the TLS handshake; extracts the SNI from the established connection via `ssl_ref.servername(NameType::HOST_NAME)` and tracks `is_mtls` in `TlsContext`.
- Calls the shared TLSRoute selection path to select a route and eligible owner.
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

Both real proxy loops use one actual-unit selection/resolution attempt helper.
Terminal denial or invalidity returns from the connection; only valid live
endpoint/connect failure can retry. Endpoint failure keeps its existing no-sleep
behavior; connect failures retain the attempt cap and backoff. No BackendTLSPolicy
capture or upstream TLS is added, and termination remains a deferred product mode.

## Key source files

| File | Responsibility |
|------|----------------|
| `edgion-gateway/src/routes/tls/routes_mgr.rs` | GlobalTlsRouteManagers, TlsRouteManager |
| `edgion-gateway/src/routes/tls/gateway_tls_routes.rs` | TlsRouteTable (SNI matching) |
| `edgion-gateway/src/routes/tls/proxy.rs` | EdgionTlsTcpProxy (TLS-terminate-to-TCP) |
| `edgion-gateway/src/routes/tls/passthrough_proxy.rs` | EdgionTlsPassthroughProxy (TLS passthrough) |
| `edgion-gateway/src/routes/tls/sni_parser.rs` | raw-ClientHello SNI extraction (passthrough) |
| `edgion-gateway/src/routes/tls/conf_handler_impl.rs` | ConfHandler<TLSRoute> implementation |
