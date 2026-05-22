---
name: gateway-runtime-store
description: Runtime stores — Gateway resource management, route configuration storage, ArcSwap lock-free reads, listener configuration, error response generation.
---

# Runtime stores

> The runtime store module manages Gateway resource configuration, route managers, and listener information,
> using ArcSwap for lock-free reads and supporting dynamic configuration updates at runtime.

## Gateway Store

`GatewayStore` stores Gateway CRD resources:

```rust
pub struct GatewayStore {
    gateways: HashMap<String, Gateway>,
}
```

Operations:
- `add_gateway()` — add (returns an error if the key already exists).
- `update_gateway()` — update or insert.
- `remove_gateway()` — remove.
- `get_gateway()` — query by key.
- `list_gateways()` — list all.
- `clear()` — clear.

Global singleton: `get_global_gateway_store()` returns `Arc<RwLock<GatewayStore>>`.

## Gateway configuration store (GatewayConfigStore)

`GatewayConfigStore` provides per-Listener Gateway configuration queries with a two-level matching structure:

```rust
pub struct GatewayListenerConfig {
    host_map: Option<HashMap<String, Arc<ListenerConfig>>>,           // exact hostname match
    wildcard_host_map: Option<HashHost<Arc<ListenerConfig>>>,         // wildcard hostname match
    listener_map: Option<HashMap<String, Arc<ListenerConfig>>>,       // match by listener name
}
```

`ListenerConfig` includes:
- `name` — Listener name.
- `port` — listening port.
- `hostname` — optional SNI hostname.
- `allowed_routes` — allowed route configuration.

Route matching strategies:
- **Routes with a sectionName**: exact match against `listener_map` by listener name.
- **Routes without a sectionName**: match by hostname against `host_map` (exact) or `wildcard_host_map` (wildcard).

Performance optimization: all internal HashMaps use `Option`; most Gateways do not configure a hostname, so the host_map lookup can be skipped.

Global singleton: `get_global_gateway_config_store()`.

## ListenerGatewayInfo Store

`ListenerGatewayInfoStore` manages an aggregated Gateway info view for each Listener, indexed by `ListenerKey` (formatted as `"{addr}/{protocol}"`):

- `get_listener_gateway_info_store()` — get the global instance.
- `rebuild_listener_gateway_infos()` — rebuild the Listener-Gateway mapping.
- Provides `GatewayInfo` (containing the aggregated runtime info of the Gateway).

## ArcSwap lock-free read pattern

Runtime stores use the ArcSwap pattern extensively for high-performance concurrent access:

```
Write path (low frequency):              Read path (high frequency):
ConfHandler.full_set()                   store.load()  -> Arc<T>
  -> build new HashMap                      -> zero-cost atomic read
  -> ArcSwap.store(Arc::new(map))           -> lock-free, copy-free
```

Stores using ArcSwap:
- `GatewayConfigStore` — Gateway listener configuration.
- `LinkSysStore` — LinkSys resources.
- `PluginStore` — EdgionPlugins resources.
- Each provider's runtime store (Redis / Etcd / ES).
- `BackendSelector` — backend selector state.

Difference vs RwLock: `GatewayStore` uses `Arc<RwLock<GatewayStore>>` (writes need mutual exclusion), suitable for higher write-frequency scenarios.

## Error response generation

The Gateway runtime provides standardized error response generators:

| Function | Status code | Use case |
|----------|-------------|----------|
| `end_response_400()` | 400 Bad Request | Malformed request |
| `end_response_404()` | 404 Not Found | No route matched |
| `end_response_421()` | 421 Misdirected Request | SNI mismatch |
| `end_response_500()` | 500 Internal Server Error | Internal error |
| `end_response_503()` | 503 Service Unavailable | No backend available |

## Directory layout

```
src/core/gateway/runtime/
└── store/
    ├── mod.rs                # module exports
    ├── gateway.rs            # GatewayStore (CRUD for Gateway resources)
    ├── config.rs             # GatewayConfigStore (Listener configuration)
    └── listener_gateway_info.rs  # ListenerGatewayInfoStore (Listener-Gateway mapping)
```
