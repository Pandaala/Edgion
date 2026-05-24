---
name: center-aggregator-watch-cache
description: ResourceAggregator, CenterWatchCache/Registry, CenterMetaDataStore, and the reverse-Watch data flow architecture.
---

# Aggregator, Watch Cache, and Metadata Store

## ResourceAggregator

Source: `src/core/center/aggregator/mod.rs`

`ResourceAggregator` (`src/core/center/aggregator/mod.rs:30-32`) tracks
per-controller registration state and drives Prometheus gauges for online controller
counts per cluster. It is the *control-plane view* of which controllers are live.

Key methods:

| Method | Effect | Source line |
|--------|--------|-------------|
| `set_controller_info(id, info)` | Insert/update a controller snapshot; clears `offline_since`; recomputes per-cluster gauges | `mod.rs:42` |
| `mark_offline(id)` | Sets `offline_since`; recomputes gauges. Entry is **retained** — offline controllers remain visible in the Admin API | `mod.rs:55` |
| `remove(id)` | Drops the snapshot entirely. Used by Admin DELETE cascade. Emits zero gauge if last controller in that cluster | `mod.rs:69` |
| `controller_summaries()` | Returns all snapshots as `Vec<ControllerSummary>` for the Admin API | `mod.rs:147` |

Internal structure: `Arc<RwLock<HashMap<String, ControllerSnapshot>>>`.
Gauge emission is done **outside** the write lock to avoid blocking registration on
any future non-trivial metrics backend (`src/core/center/aggregator/mod.rs:136-144`).

## CenterWatchCache

Source: `src/core/center/watch_cache/cache.rs`

`CenterWatchCache<T>` (`cache.rs:19-23`) is a per-controller, single-kind in-memory
cache. It mirrors Gateway's `ClientCache<T>`.

Key fields: `data: HashMap<String, Arc<T>>`, `sync_version: u64`, `server_id: String`
(inside `CacheState`, `cache.rs:25-29`).

Write paths:

- **`replace_all`** (`cache.rs:47`): Full replace from a `WatchListResponse`.
  Acquires write lock, replaces `data`, updates `sync_version` / `server_id`,
  then calls `handler.full_set()` **outside** the lock.
- **`apply_events`** (`cache.rs:64`): Incremental update from a `WatchEventResponse`.
  Classifies each event as `Add`, `Update`, or `Delete`; applies to `data` inside
  the lock; then calls `handler.partial_update()` outside the lock.

Read paths: `get_sync_version()` (`cache.rs:100`), `get_server_id()` (`cache.rs:104`).

### CenterWatchCacheRegistry

Source: `src/core/center/watch_cache/registry.rs`

`CenterWatchCacheRegistry<T>` (`registry.rs:11-13`) manages one `CenterWatchCache<T>`
per controller ID. Caches **persist across reconnects** so that `sync_version` is
preserved — on reconnect, Center sends `from_version = last_known_version` in the
`FedWatchRequest`, avoiding a full re-list when the controller was only briefly offline.

- `get_or_create(controller_id)` (`registry.rs:26`): Read-lock fast path; write-lock
  on first creation.
- `mark_offline(id)` (`registry.rs:54`): Calls `handler.controller_offline()` but
  **does not delete the cache** entry.
- `remove_controller(id)` (`registry.rs:59`): Deletes the cache; calls
  `handler.controller_removed()`.

### CenterSyncClient

Source: `src/core/center/watch_cache/mod.rs:13-15`

Top-level container: one `CenterWatchCacheRegistry<T>` field per resource kind.
Currently only `plugin_metadata: CenterWatchCacheRegistry<PluginMetaData>`.
Adding a new kind requires a new field here plus a corresponding `FedWatchRequest`.

## CenterMetaDataStore

Source: `src/core/center/metadata_store/mod.rs`

`CenterMetaDataStore` (`mod.rs:73-78`) implements `CenterConfHandler<PluginMetaData>`
and aggregates PluginMetaData across all connected controllers into three maps:

| Internal map | Key | Value |
|--------------|-----|-------|
| `cluster_routes` | `pm_key` (`"ns/name"`) | `{controller_id → ClusterRegionRouteEntry}` |
| `service_routes` | `pm_key` | `{controller_id → ServiceRegionRouteEntry}` |
| `global_ip_restrictions` | `pm_key` | `{controller_id → ControllerPmEntry}` |

Write paths (called by `CenterWatchCache` after lock release):

- **`full_set(controller_id, data)`** (`mod.rs:180`): Rebuilds all three maps for
  the given controller in a single lock-per-map pass. Old entries for that controller
  are removed first (`retain`); new classified entries are inserted.
- **`partial_update(controller_id, add, update, remove)`** (`mod.rs:256`): Applies
  add/update upserts and removes per-key, one lock per map.

Eviction semantics:

- `controller_offline` (`mod.rs:350`): **No-op** — offline data is retained so the
  Admin API can still display the last known state.
- `controller_removed` (`mod.rs:354`): Calls `remove_all_for_controller`, which
  removes every entry for that controller from all three maps.

## Reverse-Watch architecture

In a standard K8s informer, the *client* (Gateway) issues a List+Watch to the
*server* (Controller). In Center's federation, the direction is reversed:

```
Controller (data source / watch server)
    │  WatchListResponse / WatchEventResponse (push)
    ▼  ← driven by FedWatchRequest sent FROM Center
CenterWatchCache<T>   (per-controller in-memory snapshot)
    │  full_set / partial_update
    ▼
CenterMetaDataStore   (global merged view across all controllers)
    │  list_*() / get_*()
    ▼
HTTP Admin API handlers   ← auth-gated (mandatory; 503 fail-close when unconfigured;
                            see ../../02-features/02-config/03-auth-bootstrap.md)
```

Center is the **watch issuer**; Controllers are the **watch servers**. This means:
- Center can control `from_version` on (re-)watch to minimize traffic.
- Data freshness depends on the Controller's `ConfigSyncServer` being live and
  having current watch data; Center never polls K8s directly.
- On Controller restart (detected via `server_id` change in `WatchEventResponse`,
  `src/core/center/fed_sync/server/mod.rs:613-631`), Center automatically re-watches
  from version 0 to rebuild the snapshot.

The stream handler that drives `CenterWatchCache` write paths is the main loop in
`src/core/center/fed_sync/server/mod.rs:459-705` (the `WatchListResponse` and
`WatchEventResponse` arms).
