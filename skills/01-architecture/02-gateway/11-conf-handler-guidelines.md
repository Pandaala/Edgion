# ConfHandler development guidelines

> Responsibilities, classification, incremental-update constraints, and security requirements for the Gateway-side ConfHandler<T>.

## 1. What is ConfHandler

`ConfHandler<T>` is the standard interface used by the **Gateway data plane** to receive and process configuration changes (defined in `edgion-common/src/conf_sync/traits.rs`).

```
Controller → gRPC Watch/List → ClientCache → ConfHandler<T>.full_set / partial_update
```

- `full_set(&HashMap<String, T>)` — full replace, triggered by `List()` or after a gRPC reconnect.
- `partial_update(add, update, remove)` — incremental update, triggered by Watch events after the 100 ms compression window.

Each K8s / CRD resource registers one `ConfHandler` in `ConfigClient::new()`.

## 2. Resource categories

| Category | Characteristics | Representative resources | Update strategy |
|----------|-----------------|--------------------------|-----------------|
| **Storage** | Data is stored in a KV store and looked up by key per request | Service, EdgionPlugins, EdgionTls, BackendTLSPolicy, LinkSys, EdgionAcme, EdgionStreamPlugins | insert / remove suffices |
| **Engine** | Requires building a matching engine (radix tree, regex set, etc.) | HTTPRoute, GRPCRoute, TLSRoute, TCPRoute, UDPRoute | Must compute affected scope |
| **Configuration** | Low frequency, small amount of base configuration | Gateway, GatewayClass, EdgionGatewayConfig | Tolerates full rebuild |

## 3. Incremental update conventions

### 3.1 No "fake incremental"

`partial_update` MUST NOT internally merge into the full data and then call a full build equivalent to `full_set`.
If true incremental work is not possible yet, **mark with** `// TODO: incremental rebuild`.

### 3.2 Engine resources must compute affected scope

```
partial_update(add, update, remove)
  → compute affected_scope (hostname / listener_key / service_key)
  → rebuild only sub-engines in the affected scope
  → unaffected parts are reused via Arc
```

Reference implementation: HTTPRoute's `build_affected_hostnames` + per-hostname `rebuild_exact_hostname`.

### 3.3 ArcSwap is data-plane only

- The data plane (Gateway) uses `ArcSwap` for hot-path stores to enable lock-free reads.
- The control plane (Controller) does not use `ArcSwap`; `RwLock` suffices.
- Write pattern: clone current map → modify → `store(Arc::new(new_map))`.

## 4. Configuration leak protection

- ConfHandlers MUST NOT log the full content of resources (Secrets, TLS certificates, etc.).
- Logs only record metadata such as `key_name`, counts, affected scope.
- Error logs only contain the error message, never the resource spec.
- See [01-log-safety.md](../../03-coding/01-log-safety.md) for the full convention.

## 5. Notes

- `full_set` and `partial_update` execute on the same thread (the compressed-event handling thread); they are never invoked concurrently.
- `full_set` is triggered on gRPC reconnect / relist and must handle empty data (clearing state).
- `add` / `update` in `partial_update` have already been preparsed (Controller side), but Gateway-side may need additional preparse (e.g., GRPCRoute, EdgionPlugins).
- Route table updates must be atomic (`ArcSwap::store`); no "half-built" intermediate state.

## 6. Existing handler list

| Handler | Resource | File path |
|---------|----------|-----------|
| RouteManager | HTTPRoute | `routes/http/conf_handler_impl.rs` |
| GrpcRouteManager | GRPCRoute | `routes/grpc/conf_handler_impl.rs` |
| GlobalTlsRouteManagers | TLSRoute | `routes/tls/conf_handler_impl.rs` |
| GlobalTcpRouteManagers | TCPRoute | `routes/tcp/conf_handler_impl.rs` |
| GlobalUdpRouteManagers | UDPRoute | `routes/udp/conf_handler_impl.rs` |
| PluginStore | EdgionPlugins | `plugins/http/conf_handler_impl.rs` |
| StreamPluginStore | EdgionStreamPlugins | `plugins/stream/stream_plugin_store.rs` |
| TlsStore | EdgionTls | `tls/store/conf_handler.rs` |
| ServiceStore | Service | `backends/discovery/services/conf_handler_impl.rs` |
| EpSliceHandler | EndpointSlice | `backends/discovery/endpoint_slice/conf_handler_impl.rs` |
| EndpointHandler | Endpoints | `backends/discovery/endpoint/conf_handler_impl.rs` |
| BackendTLSPolicyStore | BackendTLSPolicy | `backends/policy/backend_tls/conf_handler_impl.rs` |
| GatewayHandler | Gateway | `runtime/handler.rs` |
| GatewayClassHandler | GatewayClass | `config/gateway_class/conf_handler_impl.rs` |
| EdgionGatewayConfigHandler | EdgionGatewayConfig | `config/edgion_gateway/conf_handler_impl.rs` |
| AcmeConfHandler | EdgionAcme | `services/acme/conf_handler_impl.rs` |
| LinkSysStore | LinkSys | `link_sys/runtime/conf_handler.rs` |

> All paths are relative to `src/core/gateway/`.

## 7. Best practices for incremental updates of engine resources

`partial_update` for engine resources (those that need a matching engine) should follow these patterns:

### 7.1 Rebuild by affected scope (recommended)

Suitable for resources that are bucketed across multiple dimensions (HTTPRoute by hostname, TLSRoute by listener_key).

```
partial_update:
  1. Before modifying the cache, collect old data's affected_scope (old hostname / old listener_key).
  2. Modify the cache (insert / remove).
  3. Collect new data's affected_scope (new hostname / new listener_key).
  4. Merge affected_scope = old ∪ new.
  5. Rebuild only the sub-engines within affected_scope.
```

References:
- HTTPRoute `build_affected_hostnames` → per-hostname `rebuild_exact_hostname`.
- TLSRoute `rebuild_affected_listener_managers(&affected_listeners)`.

### 7.2 Cache parsed results (suboptimal)

Suitable for global engines without bucketing dimensions (GRPCRoute).

```
partial_update:
  1. Parse only the add/update routes → update route_units_cache.
  2. Delete cache entries for removed routes.
  3. Merge all units in the cache → rebuild the engine.
```

Skips preparse + validation + unit construction for unchanged routes, and avoids deep-copying the full HashMap.

### 7.3 Short-circuit (last resort)

Suitable for resources with very few items but expensive rebuild operations (Gateway).

```
partial_update:
  if add.is_empty() && update.is_empty() && remove.is_empty() { return; }
  // ... apply changes and rebuild
```

## 8. Per-resource incremental capability rating

| Resource | Rating | Strategy |
|----------|--------|----------|
| HTTPRoute | A | Rebuild by affected hostname; unchanged hostnames are reused via Arc |
| TLSRoute | A | Rebuild by affected listener; unchanged listeners are not touched |
| TCPRoute | A | Per-route add_route / remove_route; no full rebuild |
| UDPRoute | A | Same as TCPRoute |
| EndpointSlice | A | Update LB incrementally by affected_services |
| Endpoints | A | Same as EndpointSlice |
| GRPCRoute | B | Incremental route_units_cache, but still merges all units to rebuild GrpcMatchEngine |
| Service | B | Incremental store + per-item health check reconciliation |
| EdgionPlugins | B | ArcSwap clone-modify-swap |
| EdgionTls | B | RwLock per-item insert / remove |
| EdgionStreamPlugins | B | ArcSwap clone-modify-swap |
| BackendTLSPolicy | B | ArcSwap clone-modify-swap |
| LinkSys | B | ArcSwap clone-modify-swap |
| EdgionAcme | B | Incremental challenge store |
| Gateway | B | Incremental updates to GatewayStore + ListenerGatewayInfoStore; full TLS matcher rebuild (with short-circuit optimization) |
| GatewayClass | C | Linear Vec scan, acceptable due to very small count |
| EdgionGatewayConfig | C | Same as GatewayClass |
