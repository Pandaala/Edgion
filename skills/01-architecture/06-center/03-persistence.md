---
name: center-db
description: Center federated persistence — SQLite controllers table, CenterDb struct, database configuration. The new PluginMetaData federation data flow synchronizes via a reverse Watch (CenterWatchCache → CenterMetaDataStore); there is no SQLite cache layer.
---

# Center Federated Persistence (SQLite)

## Background and core decision

Center uses SQLite to persist **controller registration information** (online/offline state, labels, etc.), ensuring that already-registered controller records are not lost when Center restarts.

**Why bundled SQLite rather than a runtime dependency:**
- `rusqlite = { features = ["bundled"] }` compiles SQLite from C source and statically links it at compile time
- No `libsqlite3.so` is needed at runtime; deploying to `Dockerfile.runtime` (ubuntu:24.04) requires no changes
- The build-side `Dockerfile.builder` (rust:bookworm, includes gcc) needs no changes either — C source compilation is handled by cargo
- Compared with the "system dynamic library" approach: bundled has zero runtime dependencies, at the cost of an additional ~30 seconds on first compile (compiling the C code)

**Concurrent-access model:**
- A single `Arc<Mutex<Connection>>` (`parking_lot::Mutex`)
- All DB operations are wrapped in `spawn_blocking` so they don't block the async runtime
- Writes are serialized (Mutex); reads are also serial (single Connection) — Center's write pressure is low, so this simplification is reasonable

## DB schema

```sql
CREATE TABLE controllers (
    controller_id TEXT PRIMARY KEY,
    cluster       TEXT NOT NULL DEFAULT '',
    env           TEXT NOT NULL DEFAULT '[]',   -- JSON array
    tag           TEXT NOT NULL DEFAULT '[]',   -- JSON array
    online        INTEGER NOT NULL DEFAULT 0,
    last_seen_at  INTEGER NOT NULL,
    created_at    INTEGER NOT NULL
);
```

Design notes:
- The `controllers` table persists controller registration metadata; Center can recover the known controller list after restart
- The `online` field is updated when a controller connects to / disconnects from the federation gRPC stream

> **Removed tables**: `cluster_plugin_metadata_cache` and `service_plugin_metadata_cache` (formerly used to cache PluginMetaData polling results) were removed together with the PM reverse-Watch refactor and no longer exist in the schema.

## Configuration (edgion-center.yaml)

```yaml
database:
  enabled: true
  sqlite_path: data/center.db  # Relative path, based on work_dir
```

- When `enabled: false`, all DB-dependent Admin APIs (such as `GET /admin/controllers`) return 503

## PluginMetaData data flow: reverse-Watch architecture

PluginMetaData (cluster/service-dimensional routing metadata) is no longer cached in SQLite; it goes through **federation gRPC bidirectional-stream reverse Watch**:

```
Controller (data source)
    │  FedWatchEventResponse (push)
    ▼
CenterSyncClient (per-controller gRPC stream client)
    │
    ▼
CenterWatchCache<T> (per-controller in-memory cache, strongly typed)
    │
    ▼
CenterMetaDataStore (global aggregated view, merged across controllers)
    │
    ▼
API Handler (reads memory directly, no SQLite)
```

Key properties:
- **Real-time push**: controller resource changes are pushed to Center immediately via FedWatchEventResponse, with no polling delay
- **No SQLite PM cache**: PluginMetaData lives entirely in memory; when a controller goes offline, its data is removed from the aggregated view (no historical snapshot is retained)
- **Strong typing**: `CenterWatchCache<T>` is generic; `T` currently includes ClusterRegionRoute PM and ServiceRegionRoute PM

For detailed design, see the project design specs (if there is an ADR document, look under `docs/` or `tasks/`).

## Admin API

> All Admin API endpoints are auth-gated (authentication is mandatory; 503 fail-close when unconfigured). See [`../../02-features/02-config/03-auth-bootstrap.md`](../../02-features/02-config/03-auth-bootstrap.md).

All endpoints return 503 when `database.enabled = false`.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v1/center/admin/controllers` | List controller records persisted in the DB (including offline records) |
| `DELETE` | `/api/v1/center/admin/controllers/{id}` | Delete the specified controller record and all associated data |
| `GET` | `/api/v1/center/cluster-region-routes` | Real-time aggregated cluster-dimensional RegionRoute PM (from memory, not SQLite) |
| `GET` | `/api/v1/center/service-region-routes` | Real-time aggregated service-dimensional RegionRoute PM (from memory, not SQLite) |
| `GET` | `/api/v1/center/cluster-region-routes/consistency` | Cluster-dimensional RegionRoute consistency check |
| `GET` | `/api/v1/center/service-region-routes/consistency` | Service-dimensional RegionRoute consistency check |

**Typical operational scenarios:**
- Permanently retire a controller → `DELETE /admin/controllers/{id}` to remove its persisted record
- View current real-time PM data → `GET /cluster-region-routes` or `/service-region-routes` (in-memory data, real-time)

**Offline / online marking (persistent, completed in 2026-04 #20):**
- Controller register → `controllers` table `upsert(online=true, last_seen_at=now, cluster/env/tag)`
- Heartbeat timeout / stream error / stream closed → `UPDATE online=0, last_seen_at=now` (narrow update; does not touch cluster/env/tag)
- Writes are async (`tokio::spawn_blocking`); on DB failure only a warning is logged, the federation stream is not blocked
- The `GET /api/v1/resources` aggregated view filters out keys for offline controllers (to avoid stale data being misleading); `GET /admin/controllers` lists all history (including offline)
- `DELETE /admin/controllers/{id}` performs a **5-way cascade**: `ControllerRegistry` + `ResourceAggregator` + `CenterWatchCacheRegistry` + `CenterMetaDataStore` (cleaned up indirectly via the `controller_removed` handler chain) + DB row
- After the Center process restarts, the Admin API can recover the controller history list from the DB; in-memory structures are rebuilt as controllers reconnect

## Docker build notes

`rusqlite bundled` does all of its work at **compile time**:
- `Dockerfile.builder` (rust:bookworm + gcc): no extra packages required, gcc is already included
- `Dockerfile.runtime` (ubuntu:24.04 + libssl3): no need to install `libsqlite3`; SQLite is statically linked into the binary

The only impact is that the first full compile takes ~30 seconds longer (compiling the C code); subsequent builds hit the cargo cache and do not recompile.

## Source locations

| Module | Path |
|--------|------|
| DB initialization & connection management | `src/core/center/db/mod.rs` |
| Schema CREATE TABLE SQL | `src/core/center/db/mod.rs` (schema inlined) |
| CenterDb (controller upsert/delete) | `src/core/center/db/mod.rs` |
| Admin API handlers | `src/core/center/api/` (`region_route_handlers.rs`, `consistency_handlers.rs`, `global_connection_ip_restriction_handlers.rs`) |
| Center configuration struct | `src/core/center/config/mod.rs` (`DatabaseConfig` field) |
| **Reverse-Watch cache layer** | `src/core/center/watch_cache/` (CenterWatchCache, CenterWatchCacheRegistry, CenterSyncClient, CenterConfHandler) |
| **PM aggregated view** | `src/core/center/metadata_store/` (CenterMetaDataStore) |
| **Shared parsing utilities** | `src/core/common/metadata_conf_handler.rs` (parse_region_route(), etc.) |
