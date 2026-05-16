---
name: center-overview
description: Deployment shape, top-level module map, startup lifecycle, and failure modes for edgion-center.
---

# Center Overview

## Deployment shape

`edgion-center` is a standalone process separate from any Controller or Gateway instance.
It binds two ports (defaults from `src/core/center/config/mod.rs:72-77`):

| Port | Protocol | Purpose |
|------|----------|---------|
| `:50052` | gRPC (optionally mTLS) | `FederationSync` — Controllers connect here |
| `:5810` | HTTP | Admin API + auth endpoints |

TLS posture for the gRPC port is governed by `grpc_security` in `CenterConfig`
(`src/core/center/config/mod.rs:44-47`). Omitting that section keeps plaintext gRPC
(backward compatible); setting it enables mTLS via `ConfSyncSecurityConfig`.
The HTTP Admin API uses axum + `compose_admin_routes` and is protected by `local_auth`
or OIDC `auth`; if neither is configured, all business routes return 503 (fail-close,
`src/core/center/cli/mod.rs:189-191`).

## Top-level module map

| Module path | Responsibility |
|-------------|----------------|
| `src/core/center/cli/mod.rs` | `EdgionCenterCli::run()` — top-level startup, wires all subsystems, spawns gRPC + HTTP tasks |
| `src/core/center/config/mod.rs` | `CenterConfig`, `CenterServerConfig`, `CenterSyncConfig`, `DatabaseConfig` |
| `src/core/center/fed_sync/server/mod.rs` | `FederationGrpcServer` — implements `FederationSync` gRPC service |
| `src/core/center/fed_sync/registry/mod.rs` | `ControllerRegistry` — live session map (stream senders, last-seen) |
| `src/core/center/aggregator/mod.rs` | `ResourceAggregator` — per-cluster online/offline controller counts |
| `src/core/center/watch_cache/` | `CenterWatchCache`, `CenterWatchCacheRegistry`, `CenterSyncClient` — per-controller typed caches |
| `src/core/center/metadata_store/mod.rs` | `CenterMetaDataStore` — global aggregated PM view across all controllers |
| `src/core/center/db/mod.rs` | `CenterDb` — SQLite persistence for controller registration records |
| `src/core/center/api/mod.rs` | Axum router + `ApiState` for the HTTP Admin API |
| `src/core/center/commander/mod.rs` | `Commander` — fan-out command dispatch to controllers |
| `src/core/center/proxy/mod.rs` | `ProxyForwarder` — HTTP proxy requests forwarded to controllers |

## Lifecycle

### Startup sequence (`src/core/center/cli/mod.rs:29-175`)

1. Parse config from `edgion-center.yaml` (defaults used if missing or unparseable).
2. Install tracing subscriber + Prometheus recorder.
3. Construct shared state: `ControllerRegistry`, `ResourceAggregator`, `CenterMetaDataStore`,
   `CenterSyncClient` (wires `CenterWatchCacheRegistry` → `CenterMetaDataStore` handler).
4. Optionally open SQLite (skipped when `database.enabled = false`).
5. Build `FederationGrpcServer` with optional TLS config.
6. Spawn gRPC task (`tonic::transport::Server`) and HTTP task (`axum::serve`).
7. `tokio::select!` waits for Ctrl-C or either task to exit (`src/core/center/cli/mod.rs:232-238`).

### Registration acceptance

When a Controller dials in (`src/core/center/fed_sync/server/mod.rs:218-260`):
1. Wait up to 5 s for first `ControllerMessage` (must be `RegisterRequest`).
2. Run `validate_register_req` + `registry_capacity_exceeded` — any failure returns
   a `Status` error and leaves zero residue in in-memory state or SQLite.
3. Register in `ControllerRegistry` + `ResourceAggregator`; async-upsert to SQLite.
4. Send `RegisterAck`, then immediately send `FedWatchRequest` for `PluginMetaData`.
5. Spawn heartbeat task (Ping every `ping_interval_secs`, default 30 s).
6. Enter main message loop.

### Ongoing sync

The main loop (`src/core/center/fed_sync/server/mod.rs:459-705`) forwards:
- `Pong` — no-op, but refreshes `last_seen`.
- `WatchListResponse` / `WatchEventResponse` — applied to `CenterWatchCache`, which
  calls `CenterMetaDataStore` via the `CenterConfHandler` trait.
- `CommandResponse` / `HttpProxyResponse` — routed to pending one-shot channels.

## Failure modes

| Scenario | Code behavior |
|----------|---------------|
| Controller disconnects (stream closed or error) | `mark_offline_all` called: `ControllerRegistry::mark_offline`, `ResourceAggregator::mark_offline`, `CenterWatchCacheRegistry::mark_offline`, async `CenterDb::mark_offline`. `CenterMetaDataStore` data retained (offline data stays queryable). `src/core/center/fed_sync/server/mod.rs:414-447` |
| Heartbeat timeout (3× `ping_interval`) | Same `mark_offline_all` path, `reason = HEARTBEAT`. `src/core/center/fed_sync/server/mod.rs:460-468` |
| Center restarts | In-memory state is empty; Controllers reconnect and re-register automatically. SQLite preserves the controller registry (if enabled); `CenterMetaDataStore` is rebuilt from incoming watch data on reconnect. |
| Registry at capacity (10 000 entries) | New controller IDs are rejected with `Status::resource_exhausted`; reconnects from already-known IDs are still accepted. `src/core/center/fed_sync/server/mod.rs:252-260` |
| SQLite unavailable at startup | Center logs an error and continues without persistence; in-memory structures remain fully operational. `src/core/center/cli/mod.rs:91-104` |
| SQLite write failure during registration | Warning is logged; federation stream is not blocked. `src/core/center/fed_sync/server/mod.rs:287-295` |
