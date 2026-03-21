---
name: link-controller-side
description: Controller 侧 gRPC 实现：ConfigSyncServer、ConfigSyncGrpcServer、ConfigSyncServerProvider、WatchObj trait、ClientRegistry。
---

# Controller 侧实现

Controller 通过 `ConfigSyncServer` + `ConfigSyncGrpcServer` 对外提供 gRPC 配置同步服务。ResourceProcessor 将处理后的资源写入 `ServerCache<T>`，ServerCache 实现 `WatchObj` trait 提供统一的 list/watch 接口，ConfigSyncServer 聚合所有 WatchObj 并通过 gRPC 暴露给 Gateway。

## ConfigSyncServer

gRPC 配置同步服务器的核心状态管理器，持有所有资源的 WatchObj 并提供统一的 list/watch API。

```rust
pub struct ConfigSyncServer {
    server_id: RwLock<String>,                              // 服务器实例 ID（毫秒时间戳）
    endpoint_mode: RwLock<Option<EndpointMode>>,             // 端点发现模式
    watch_objects: RwLock<HashMap<String, Arc<dyn WatchObj>>>, // 按 kind 注册的 WatchObj
    client_registry: Arc<ClientRegistry>,                    // 已连接 Gateway 实例注册表
}
```

### server_id

启动时生成的毫秒时间戳字符串，用于客户端检测服务器重启。当 Gateway 收到的 `server_id` 与之前不同时，触发全量 relist。重链接（relink）时调用 `regenerate_server_id()` 生成新 ID。

### WatchObj 注册

ResourceProcessor 初始化时通过 `register_watch_obj(kind, Arc<ServerCache<T>>)` 向 ConfigSyncServer 注册。所有注册完成且就绪后（`is_all_ready()` 返回 true），gRPC 服务开始对外提供数据。

### list/watch 操作

| 操作 | 流程 |
|------|------|
| `list(kind)` | 查找 WatchObj → `list_json()` → 返回 `ListDataSimple { data, sync_version, server_id }` |
| `watch(kind, client_id, ...)` | 查找 WatchObj → `watch_json()` → 启动转换任务附加 server_id → 返回 `mpsc::Receiver<EventDataSimple>` |

Watch 操作启动一个 tokio 任务，将 `WatchResponseSimple` 转换为 `EventDataSimple`（附加 server_id），确保客户端能检测服务器重启。

### 状态管理

| 方法 | 说明 |
|------|------|
| `is_all_ready()` | 所有 WatchObj 均已就绪 |
| `not_ready_kinds()` | 返回尚未就绪的 kind 列表 |
| `set_all_not_ready()` | 重链接时标记所有缓存未就绪 |
| `clear_all()` | 清空所有缓存数据 |
| `reset_for_relink()` | 完整重置：set_all_not_ready + clear_all + regenerate_server_id |

## ConfigSyncGrpcServer

实现 `ConfigSync` proto service 的 gRPC 服务层，负责接收 Gateway 的 gRPC 调用并路由到 ConfigSyncServer。

```rust
pub struct ConfigSyncGrpcServer {
    provider: Arc<dyn ConfigSyncServerProvider>,
}
```

ConfigSyncGrpcServer 不直接持有 ConfigSyncServer，而是通过 `ConfigSyncServerProvider` trait 动态获取最新实例，以支持 reload 场景下的服务器实例切换。

### 4 个 RPC 实现

| RPC | 实现逻辑 |
|-----|---------|
| `GetServerInfo` | 返回 server_id、endpoint_mode、supported_kinds（所有已注册的 WatchObj kind） |
| `List` | 校验 expected_server_id → 查找对应 kind 的 WatchObj → 调用 `list_json()` → 返回 ListResponse |
| `Watch` | 校验 expected_server_id → 查找 WatchObj → 调用 `watch_json()` → 返回 streaming WatchResponse |
| `WatchServerMeta` | 注册 Gateway 实例到 ClientRegistry → 推送 gateway_instance_count 变更事件流 |

当 `expected_server_id` 与当前 `server_id` 不匹配时，返回 `WATCH_ERR_SERVER_ID_MISMATCH` 错误，通知 Gateway 重新同步。

## ConfigSyncServerProvider

Provider trait，允许 gRPC 服务动态获取最新的 ConfigSyncServer 实例。

```rust
pub trait ConfigSyncServerProvider: Send + Sync {
    fn config_sync_server(&self) -> Option<Arc<ConfigSyncServer>>;
}
```

这种设计解决了 reload 场景下的服务器实例切换问题：Controller 执行 reload 时会创建新的 ConfigSyncServer 实例（新的 server_id、新的 WatchObj 集合），而 gRPC 服务层通过 provider 透明地切换到新实例，无需重启 gRPC 服务。

返回 `None` 表示 ConfigSyncServer 尚未就绪（如 Controller 正在初始化或 reload 中），gRPC 层返回 `Status::unavailable`。

## 注册时序要求

ConfigSyncServer 必须等待所有分阶段处理器（phased processors）完成注册后才能发布。

在 Kubernetes 模式下，Phase 1 基础资源（Gateway、Service、Endpoints 等）先于 Phase 2 路由/TLS/插件资源启动。如果在 Phase 1 就绪后立即发布 ConfigSyncServer，Gateway 可能收到不完整的 `supported_kinds`，导致对 Phase 2 资源的 List 调用返回错误：

```
Failed to list resources: Unknown kind: HTTPRoute
Failed to list resources: Unknown kind: GRPCRoute
Failed to list resources: Unknown kind: EdgionTls
```

这属于控制面就绪性问题，而非资源类型不支持。Gateway 可能仍然使用之前缓存的快照继续服务流量。

## WatchObj trait

对象安全（object-safe）的 list/watch 接口，由 `ServerCache<T>` 实现，使 ConfigSyncServer 能以统一方式管理不同类型的缓存。

```rust
pub trait WatchObj: Send + Sync {
    fn kind_name(&self) -> &'static str;
    fn list_json(&self) -> Result<(String, u64), String>;
    fn watch_json(&self, client_id: String, client_name: String, from_version: u64)
        -> mpsc::Receiver<WatchResponseSimple>;
    fn is_ready(&self) -> bool;
    fn set_ready(&self);
    fn set_not_ready(&self);
    fn clear(&self);
}
```

所有方法涉及 JSON 序列化，因此使用 trait object 是合理的——序列化开销远大于虚调用开销。

| 方法 | 说明 |
|------|------|
| `kind_name()` | 返回资源类型名称（如 "Gateway"、"HTTPRoute"） |
| `list_json()` | 返回全量资源的 JSON 字符串 + 当前 sync_version |
| `watch_json()` | 启动 watcher 任务，返回 `mpsc::Receiver` 持续接收变更事件流 |
| `is_ready()` | 缓存是否已完成初始化 |
| `set_ready()` / `set_not_ready()` | 切换就绪状态 |
| `clear()` | 清空缓存数据 |

## ClientRegistry

追踪已连接的 Gateway 实例，维护 gateway_instance_count 信息。

主要用途：
- **集群级限流**：Gateway 通过 WatchServerMeta 接收 `gateway_instance_count`，用于计算集群级别的限流配额（单实例配额 = 集群总配额 / 实例数）
- **实例监控**：Controller 可查看当前连接的 Gateway 实例列表及其状态
- **变更通知**：当 Gateway 实例上线/下线时，通过 WatchServerMeta 流推送更新的 `gateway_instance_count` 给所有已连接的 Gateway

## ServerCache 集成

ResourceProcessor 是 ServerCache 的数据源，完整的数据流：

```
K8s API / FileSystem
        │
        ▼
  ConfCenter (watcher)
        │
        ▼
  ResourceProcessor.process_resource()    ← 11 步处理流水线
        │
        ▼
  ServerCache.apply_change()              ← 分配 sync_version，写入 EventStore
        │
        ▼
  Notify.notify_waiters()                 ← 唤醒所有 watcher 任务
        │
        ▼
  Watcher Task → mpsc::channel            ← 发送增量事件
        │
        ▼
  ConfigSyncServer.watch()                ← 附加 server_id
        │
        ▼
  gRPC Stream → Gateway                  ← Gateway 实例接收配置变更
```

ServerCache 的详细实现（EventStore、环形缓冲区、sync_version 机制）见 [01-controller/07-cache-server.md](../01-controller/07-cache-server.md)。

## 模块结构

```
src/core/controller/conf_sync/
├── conf_server/
│   ├── mod.rs                  # 模块导出
│   ├── config_sync_server.rs   # ConfigSyncServer 核心逻辑
│   ├── grpc_server.rs          # ConfigSyncGrpcServer + ConfigSyncServerProvider
│   ├── client_registry.rs      # ClientRegistry 实现
│   └── traits.rs               # WatchObj trait + WatchResponseSimple
└── cache_server/
    ├── cache.rs                # ServerCache<T> 泛型缓存
    ├── store.rs                # EventStore<T> 环形事件缓冲区
    └── types.rs                # WatcherEvent 等类型定义
```
