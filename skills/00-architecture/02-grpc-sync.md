# gRPC 配置同步

> Controller ↔ Gateway 之间的 gRPC 通信机制：Proto 定义、同步流程、Server/Client 端实现。
> `ReferenceGrant` 和 `Secret` 是 `no_sync_kinds`，不会发送到 Gateway。

## Proto Definition

`src/core/common/conf_sync/proto/config_sync.proto`:

```protobuf
service ConfigSync {
    rpc GetServerInfo(ServerInfoRequest) returns (ServerInfoResponse);
    rpc List(ListRequest) returns (ListResponse);
    rpc Watch(WatchRequest) returns (stream WatchResponse);
    rpc WatchServerMeta(WatchServerMetaRequest) returns (stream ServerMetaEvent);
}
```

## Sync Flow

```
Gateway startup:
  1. GetServerInfo() → server_id, endpoint_mode, supported_kinds
  2. For each kind: List(kind) → full snapshot
  3. For each kind: Watch(kind, from_version) → streaming updates

Controller reload:
  1. Controller generates new server_id
  2. Watch stream sends WATCH_ERR_SERVER_RELOAD
  3. Gateway detects server_id change
  4. Gateway re-Lists all kinds (full re-sync)
```

## Server Side (Controller)

```
PROCESSOR_REGISTRY
  → all_watch_objs(no_sync_kinds)     # Builds WatchObj per kind
    → ConfigSyncServer { watch_objs }
      → ConfigSyncGrpcServer serves List/Watch
        → ConfigSyncServerProvider for reload (swap server on reload)
```

`ReferenceGrant` and `Secret` are `no_sync_kinds` — not sent to Gateway.

## Client Side (Gateway)

```
ConfigSyncClient
  → per-kind ClientCache<T>
    → Watch stream → ConfHandler { full_set, partial_update }
      → cache_data updated (ArcSwap for lock-free reads)
      → preparse triggered on update
```

## Module Split

- `src/core/common/conf_sync/` — gRPC proto、shared traits、shared event/list/watch types
- `src/core/controller/conf_sync/` — Controller 侧 `ConfigSyncServer`、`ServerCache`
- `src/core/gateway/conf_sync/` — Gateway 侧 `ConfigSyncClient`、`ClientCache`

**Key files:**
- `src/core/common/conf_sync/proto/config_sync.proto` — proto definition
- `src/core/controller/conf_sync/conf_server/` — gRPC server, `ConfigSyncServer`
- `src/core/gateway/conf_sync/conf_client/grpc_client.rs` — `ConfigSyncClient`
- `src/core/gateway/conf_sync/cache_client/cache.rs` — `ClientCache<T>`, `DynClientCache`
