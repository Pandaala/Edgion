---
name: link-overview
description: Controller↔Gateway gRPC 同步协议：Proto 定义、同步流程、版本追踪、资源变更事件、模块分布。
---

# 同步协议总览

Controller 和 Gateway 通过 `ConfigSync` gRPC 服务实现配置同步。Gateway 启动后作为客户端连接 Controller，先获取服务器信息，再对每种资源执行 List（全量）+ Watch（增量）同步。

## Proto 定义

Proto 文件位于 `src/core/common/conf_sync/proto/config_sync.proto`：

```protobuf
service ConfigSync {
    rpc GetServerInfo(ServerInfoRequest) returns (ServerInfoResponse);
    rpc List(ListRequest) returns (ListResponse);
    rpc Watch(WatchRequest) returns (stream WatchResponse);
    rpc WatchServerMeta(WatchServerMetaRequest) returns (stream ServerMetaEvent);
}
```

### 4 个 RPC 方法

| RPC | 方向 | 说明 |
|-----|------|------|
| **GetServerInfo** | 一元 | 返回 `server_id`、`endpoint_mode`（EndpointSlice/Endpoint/Auto）、`supported_kinds` 列表 |
| **List** | 一元 | 按 kind 获取全量资源快照，返回 JSON 数据 + `sync_version` + `server_id` |
| **Watch** | 服务端流 | 按 kind 持续接收增量变更事件，携带 `from_version` 实现续接 |
| **WatchServerMeta** | 服务端流 | 接收服务器元数据变更（`gateway_instance_count`），用于集群级感知 |

### 关键消息字段

| 消息 | 关键字段 |
|------|---------|
| `ListRequest` | `kind`、`expected_server_id` |
| `ListResponse` | `data`（JSON）、`sync_version`、`server_id` |
| `WatchRequest` | `kind`、`client_id`、`client_name`、`from_version`、`expected_server_id` |
| `WatchResponse` | `data`（JSON）、`sync_version`、`err`、`server_id` |
| `ServerInfoResponse` | `server_id`、`endpoint_mode`、`supported_kinds` |
| `ServerMetaEvent` | `server_id`、`gateway_instance_count`、`timestamp` |

## 同步流程

### Gateway 启动同步

```
Gateway 启动:
  1. GetServerInfo() → 获取 server_id, endpoint_mode, supported_kinds
  2. 对每种 kind: List(kind) → 全量快照（InitStart → InitAdd × N → InitDone）
  3. 对每种 kind: Watch(kind, from_version) → 持续接收增量变更
```

Gateway 通过 `supported_kinds` 确定需要同步哪些资源类型，然后逐一发起 List 和 Watch 调用。List 返回的 `sync_version` 作为 Watch 的 `from_version`，实现无缝衔接。

### Controller 重载流程

```
Controller 重载（reload）:
  1. Controller 生成新的 server_id
  2. Watch 流发送 WATCH_ERR_SERVER_RELOAD 错误
  3. Gateway 检测到 server_id 变化
  4. Gateway 对所有 kind 重新执行 List（全量重同步）
```

当 Controller 执行 reload 操作（如 K8s 模式下重新连接 API Server）时，会生成新的 `server_id` 并通过 Watch 流通知所有 Gateway 实例。Gateway 收到错误后触发全量 relist，确保配置一致性。

### server_id 不匹配检测

List 和 Watch 请求均携带 `expected_server_id`。如果 Gateway 发送的 `expected_server_id` 与 Controller 当前 `server_id` 不一致，Controller 返回 `WATCH_ERR_SERVER_ID_MISMATCH` 错误，Gateway 将重新获取 ServerInfo 并执行全量同步。

## 版本追踪

| 概念 | 类型 | 说明 |
|------|------|------|
| `sync_version` | `u64` | 全局单调递增版本号，通过 `utils::next_resource_version()` 生成，每次写入操作分配新值 |
| `server_id` | `String` | Controller 实例 ID（毫秒时间戳），每次启动或 reload 时重新生成 |
| `from_version` | `u64` | Gateway 侧记录的最后同步版本，用于 Watch 续接 |
| `expected_server_id` | `String` | Gateway 在 List/Watch 请求中携带，用于检测 Controller 重启 |

版本追踪机制确保：
- **增量同步**：Watch 从 `from_version` 之后开始推送事件，避免重复传输
- **过期检测**：当 `from_version` 太旧（事件已被环形缓冲区覆盖）时，返回 `TooOldVersion` 错误，触发全量重同步
- **实例识别**：`server_id` 变化表示 Controller 重启或重载，Gateway 必须全量重同步

## 资源变更事件

资源变更通过 `ResourceChange` 枚举表示，定义在 `src/core/common/conf_sync/traits.rs` 中：

| 事件类型 | 阶段 | 说明 |
|---------|------|------|
| `InitStart` | 初始化 | 信号：初始化开始（仅日志，不携带资源数据） |
| `InitAdd` | 初始化 | 初始化期间添加资源（写入 EventStore 的 data，不进入环形缓冲区） |
| `InitDone` | 初始化 | 信号：初始化完成，缓存可标记为就绪 |
| `EventAdd` | 运行时 | 运行时新增资源（写入环形缓冲区 + 通知 watcher） |
| `EventUpdate` | 运行时 | 运行时更新资源（写入环形缓冲区 + 通知 watcher） |
| `EventDelete` | 运行时 | 运行时删除资源（写入环形缓冲区 + 通知 watcher） |

初始化阶段的事件（InitStart/InitAdd/InitDone）不进入环形缓冲区，仅更新全量快照。运行时事件（EventAdd/EventUpdate/EventDelete）同时写入环形缓冲区和更新快照，并触发 watcher 通知。

## 不同步的资源（no_sync_kinds）

以下资源类型不会通过 gRPC 同步到 Gateway：

| 资源 | 原因 |
|------|------|
| `ReferenceGrant` | 仅用于 Controller 侧的跨命名空间引用校验 |
| `Secret` | 仅用于 Controller 侧的证书/密钥处理，解析后的 TLS 数据通过 EdgionTls 同步 |

`no_sync_kinds` 在 `ProcessorRegistry.all_watch_objs()` 调用时排除这些资源，它们不会出现在 `supported_kinds` 列表中。

## 模块分布

| 模块 | 路径 | 职责 |
|------|------|------|
| **common/conf_sync** | `src/core/common/conf_sync/` | Proto 定义、共享 traits（`CacheEventDispatch`、`ConfHandler`、`ResourceChange`）、共享事件/列表/Watch 类型 |
| **controller/conf_sync** | `src/core/controller/conf_sync/` | 服务端实现：`ConfigSyncServer`、`ConfigSyncGrpcServer`、`ClientRegistry`、`WatchObj` trait |
| **gateway/conf_sync** | `src/core/gateway/conf_sync/` | 客户端实现：`ConfigSyncClient`、`ClientCache<T>`、`CacheData`、event_dispatch |

### 关键文件

- `src/core/common/conf_sync/proto/config_sync.proto` — Proto 定义
- `src/core/common/conf_sync/traits.rs` — `ResourceChange`、`CacheEventDispatch`、`ConfHandler`
- `src/core/controller/conf_sync/conf_server/config_sync_server.rs` — `ConfigSyncServer`
- `src/core/controller/conf_sync/conf_server/grpc_server.rs` — `ConfigSyncGrpcServer`、`ConfigSyncServerProvider`
- `src/core/controller/conf_sync/conf_server/client_registry.rs` — `ClientRegistry`
- `src/core/controller/conf_sync/conf_server/traits.rs` — `WatchObj`、`WatchResponseSimple`
- `src/core/gateway/conf_sync/conf_client/grpc_client.rs` — `ConfigSyncClient`
- `src/core/gateway/conf_sync/cache_client/cache.rs` — `ClientCache<T>`、`DynClientCache`
- `src/core/gateway/conf_sync/cache_client/event_dispatch.rs` — 事件分发逻辑
